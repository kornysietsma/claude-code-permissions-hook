//! Shell command analysis: splits a command into segments (simple commands) and records the
//! constructs that need a closer look. Pure: no policy and no I/O.

mod words;

use brush_parser::ast::{
    Command, CommandPrefixOrSuffixItem, CompoundCommand, CompoundList, IoFileRedirectKind,
    IoFileRedirectTarget, IoRedirect, Pipeline, RedirectList, SimpleCommand, Word,
};
use brush_parser::{Parser, ParserOptions};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

/// One simple command, as rules see it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    /// `name` and `args`, re-quoted only where needed; what rules usually match
    pub text: String,
    pub name: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub redirects: Vec<Redirect>,
    pub wrappers: Vec<String>,
    /// The command as parsed, for the audit log
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Redirect {
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fd: Option<i32>,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructKind {
    ParseError,
    Unsupported,
}

impl ConstructKind {
    pub fn name(self) -> &'static str {
        match self {
            ConstructKind::ParseError => "parse_error",
            ConstructKind::Unsupported => "unsupported",
        }
    }

    /// Floors always force at least an `ask`
    pub fn is_floor(self) -> bool {
        match self {
            ConstructKind::ParseError | ConstructKind::Unsupported => true,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ConstructKind::ParseError => "the command could not be parsed",
            ConstructKind::Unsupported => "shell syntax that tool-gate-hook can't check",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Construct {
    pub kind: ConstructKind,
    /// Index into `Analysis::segments`
    pub segment: Option<usize>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// In textual order
    pub segments: Vec<Segment>,
    pub constructs: Vec<Construct>,
}

impl Analysis {
    /// For a call whose command can't be analysed at all
    pub fn unsupported(detail: &str) -> Self {
        Analysis {
            segments: vec![],
            constructs: vec![Construct {
                kind: ConstructKind::Unsupported,
                segment: None,
                detail: Some(detail.to_owned()),
            }],
        }
    }
}

pub fn analyse(command: &str, home: &Path) -> Analysis {
    let options = ParserOptions::default();
    let mut walker = Walker {
        home,
        options: &options,
        analysis: Analysis::default(),
    };
    match Parser::new(Cursor::new(command), &options).parse_program() {
        Ok(program) => {
            for list in &program.complete_commands {
                walker.list(list, &[]);
            }
        }
        Err(e) => walker.construct(ConstructKind::ParseError, None, e.to_string()),
    }
    walker.analysis
}

struct Walker<'a> {
    home: &'a Path,
    options: &'a ParserOptions,
    analysis: Analysis,
}

impl Walker<'_> {
    /// `redirects` are inherited from enclosing groups, and apply to every command inside
    fn list(&mut self, list: &CompoundList, redirects: &[Redirect]) {
        for item in &list.0 {
            for (_, pipeline) in item.0.iter() {
                self.pipeline(pipeline, redirects);
            }
        }
    }

    fn pipeline(&mut self, pipeline: &Pipeline, redirects: &[Redirect]) {
        let wrappers: &[&str] = if pipeline.timed.is_some() {
            &["time"]
        } else {
            &[]
        };
        for command in &pipeline.seq {
            match command {
                Command::Simple(simple) => self.simple(simple, redirects, wrappers),
                Command::Compound(CompoundCommand::Subshell(group), group_redirects) => {
                    self.group(&group.list, group_redirects.as_ref(), redirects);
                }
                Command::Compound(CompoundCommand::BraceGroup(group), group_redirects) => {
                    self.group(&group.list, group_redirects.as_ref(), redirects);
                }
                Command::Compound(compound, _) => {
                    let kind = match compound {
                        CompoundCommand::Arithmetic(_) => "arithmetic command",
                        CompoundCommand::ArithmeticForClause(_) | CompoundCommand::ForClause(_) => {
                            "for loop"
                        }
                        CompoundCommand::CaseClause(_) => "case statement",
                        CompoundCommand::IfClause(_) => "if statement",
                        CompoundCommand::WhileClause(_) => "while loop",
                        CompoundCommand::UntilClause(_) => "until loop",
                        CompoundCommand::Coprocess(_) => "coprocess",
                        CompoundCommand::BraceGroup(_) | CompoundCommand::Subshell(_) => "group",
                    };
                    self.construct(ConstructKind::Unsupported, None, kind);
                }
                Command::Function(_) => {
                    self.construct(ConstructKind::Unsupported, None, "function definition");
                }
                Command::ExtendedTest(..) => {
                    self.construct(ConstructKind::Unsupported, None, "[[ … ]] test");
                }
            }
        }
    }

    fn group(&mut self, list: &CompoundList, own: Option<&RedirectList>, inherited: &[Redirect]) {
        let mut redirects = inherited.to_vec();
        for redirect in own.iter().flat_map(|own| own.0.iter()) {
            if let Some(redirect) = self.redirect(redirect, None) {
                redirects.push(redirect);
            }
        }
        self.list(list, &redirects);
    }

    fn simple(&mut self, simple: &SimpleCommand, inherited: &[Redirect], wrappers: &[&str]) {
        let index = self.analysis.segments.len();
        let Some(name_word) = &simple.word_or_name else {
            self.construct(
                ConstructKind::Unsupported,
                None,
                format!("no command word in {simple}"),
            );
            return;
        };
        let name = self.word(name_word, index);
        let mut args = vec![];
        let mut redirects = inherited.to_vec();
        let prefix = simple.prefix.iter().flat_map(|prefix| prefix.0.iter());
        let suffix = simple.suffix.iter().flat_map(|suffix| suffix.0.iter());
        let items = prefix
            .map(|item| (true, item))
            .chain(suffix.map(|item| (false, item)));
        for (in_prefix, item) in items {
            match item {
                CommandPrefixOrSuffixItem::Word(word) => args.push(self.word(word, index)),
                CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    if let Some(redirect) = self.redirect(redirect, Some(index)) {
                        redirects.push(redirect);
                    }
                }
                CommandPrefixOrSuffixItem::AssignmentWord(assignment, word) => {
                    // Prefix assignments are checked from step 5; suffix ones (`export X=1`) are args
                    if in_prefix {
                        self.construct(
                            ConstructKind::Unsupported,
                            Some(index),
                            format!("assignment {}", assignment.name),
                        );
                    } else {
                        args.push(self.word(word, index));
                    }
                }
                CommandPrefixOrSuffixItem::ProcessSubstitution(..) => {
                    self.construct(
                        ConstructKind::Unsupported,
                        Some(index),
                        "process substitution",
                    );
                }
            }
        }
        let text = std::iter::once(&name)
            .chain(&args)
            .map(|word| words::quote(word))
            .collect::<Vec<_>>()
            .join(" ");
        self.analysis.segments.push(Segment {
            text,
            name,
            args,
            env: BTreeMap::new(),
            redirects,
            wrappers: wrappers.iter().map(|w| (*w).to_owned()).collect(),
            source: simple.to_string(),
        });
    }

    /// The word's static value; otherwise the raw word, recording why it isn't static
    fn word(&mut self, word: &Word, segment: usize) -> String {
        words::static_value(&word.value, self.home, self.options).unwrap_or_else(|why| {
            self.construct(
                ConstructKind::Unsupported,
                Some(segment),
                format!("{why} in {}", word.value),
            );
            word.value.clone()
        })
    }

    fn redirect(&mut self, redirect: &IoRedirect, segment: Option<usize>) -> Option<Redirect> {
        let unsupported = |walker: &mut Self, detail: &str| {
            walker.construct(ConstructKind::Unsupported, segment, detail);
            None
        };
        match redirect {
            IoRedirect::File(fd, kind, target) => {
                let op = match kind {
                    IoFileRedirectKind::Read => "<",
                    IoFileRedirectKind::Write => ">",
                    IoFileRedirectKind::Append => ">>",
                    IoFileRedirectKind::ReadAndWrite => "<>",
                    IoFileRedirectKind::Clobber => ">|",
                    IoFileRedirectKind::DuplicateInput => "<&",
                    IoFileRedirectKind::DuplicateOutput => ">&",
                };
                let target = match target {
                    IoFileRedirectTarget::Filename(word)
                    | IoFileRedirectTarget::Duplicate(word) => self.redirect_word(word, segment),
                    IoFileRedirectTarget::Fd(fd) => fd.to_string(),
                    IoFileRedirectTarget::ProcessSubstitution(..) => {
                        return unsupported(self, "process substitution");
                    }
                };
                Some(Redirect {
                    op,
                    fd: *fd,
                    target,
                })
            }
            IoRedirect::HereDocument(fd, doc) => {
                if doc.requires_expansion {
                    return unsupported(self, "heredoc with an unquoted delimiter");
                }
                Some(Redirect {
                    op: if doc.remove_tabs { "<<-" } else { "<<" },
                    fd: *fd,
                    target: doc.here_end.value.clone(),
                })
            }
            IoRedirect::HereString(fd, word) => Some(Redirect {
                op: "<<<",
                fd: *fd,
                target: self.redirect_word(word, segment),
            }),
            IoRedirect::OutputAndError(word, append) => Some(Redirect {
                op: if *append { "&>>" } else { "&>" },
                fd: None,
                target: self.redirect_word(word, segment),
            }),
        }
    }

    fn redirect_word(&mut self, word: &Word, segment: Option<usize>) -> String {
        words::static_value(&word.value, self.home, self.options).unwrap_or_else(|why| {
            self.construct(
                ConstructKind::Unsupported,
                segment,
                format!("{why} in redirect {}", word.value),
            );
            word.value.clone()
        })
    }

    fn construct(
        &mut self,
        kind: ConstructKind,
        segment: Option<usize>,
        detail: impl Into<String>,
    ) {
        self.analysis.constructs.push(Construct {
            kind,
            segment,
            detail: Some(detail.into()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn analyse_here(command: &str) -> Analysis {
        analyse(command, Path::new("/home/me"))
    }

    fn texts(analysis: &Analysis) -> Vec<&str> {
        analysis.segments.iter().map(|s| s.text.as_str()).collect()
    }

    fn floors(analysis: &Analysis) -> Vec<(&'static str, Option<usize>, &str)> {
        analysis
            .constructs
            .iter()
            .filter(|c| c.kind.is_floor())
            .map(|c| (c.kind.name(), c.segment, c.detail.as_deref().unwrap_or("")))
            .collect()
    }

    #[test]
    fn lists_and_pipelines_become_segments_in_order() {
        let analysis = analyse_here("cargo build 2>&1 && cargo test | tee out.txt; ls -la || true");

        assert_eq!(
            texts(&analysis),
            vec!["cargo build", "cargo test", "tee out.txt", "ls -la", "true"]
        );
        assert_eq!(analysis.constructs, vec![]);
        assert_eq!(
            analysis.segments[0].redirects,
            vec![Redirect {
                op: ">&",
                fd: Some(2),
                target: "1".into()
            }]
        );
    }

    #[test]
    fn segment_fields_are_unquoted_and_text_is_requoted() {
        let analysis = analyse_here(r#"./scripts/thing.py --out "my file.txt" > log.txt"#);

        assert_eq!(
            analysis.segments,
            vec![Segment {
                text: "./scripts/thing.py --out 'my file.txt'".into(),
                name: "./scripts/thing.py".into(),
                args: vec!["--out".into(), "my file.txt".into()],
                env: BTreeMap::new(),
                redirects: vec![Redirect {
                    op: ">",
                    fd: None,
                    target: "log.txt".into()
                }],
                wrappers: vec![],
                source: r#"./scripts/thing.py --out "my file.txt" > log.txt"#.into(),
            }]
        );
    }

    #[test]
    fn groups_are_walked_and_pass_their_redirects_on() {
        let analysis = analyse_here("( cd sub && make ) 2>&1; { echo a; echo b; } > out");

        assert_eq!(texts(&analysis), vec!["cd sub", "make", "echo a", "echo b"]);
        assert_eq!(analysis.segments[1].redirects[0].op, ">&");
        assert_eq!(analysis.segments[3].redirects[0].target, "out");
    }

    #[test]
    fn time_is_recorded_as_a_wrapper() {
        let analysis = analyse_here("time cargo test");

        assert_eq!(analysis.segments[0].text, "cargo test");
        assert_eq!(analysis.segments[0].wrappers, vec!["time"]);
    }

    #[test]
    fn heredocs_with_quoted_delimiters_are_data() {
        let analysis = analyse_here("git commit -F - <<'EOF'\npip install x\nEOF");

        assert_eq!(texts(&analysis), vec!["git commit -F -"]);
        assert_eq!(analysis.constructs, vec![]);
    }

    #[test]
    fn unparseable_commands_are_a_parse_error() {
        let analysis = analyse_here("echo 'unterminated");

        assert_eq!(floors(&analysis)[0].0, "parse_error");
        assert_eq!(analysis.segments, vec![]);
    }

    #[test]
    fn control_structures_are_unsupported() {
        for (command, detail) in [
            ("if true; then echo; fi", "if statement"),
            ("for f in a b; do echo $f; done", "for loop"),
            ("while true; do :; done", "while loop"),
            ("[[ -f x ]] && echo y", "[[ … ]] test"),
            ("f() { echo; }", "function definition"),
            ("coproc foo", "coprocess"),
        ] {
            assert_eq!(
                floors(&analyse_here(command)),
                vec![("unsupported", None, detail)],
                "{command}"
            );
        }
    }

    #[test]
    fn non_static_words_are_unsupported_until_expansions_are_classified() {
        let analysis = analyse_here("echo $HOME && ls *.rs");

        assert_eq!(texts(&analysis), vec!["echo '$HOME'", "ls '*.rs'"]);
        assert_eq!(
            floors(&analysis),
            vec![
                ("unsupported", Some(0), "variable in $HOME"),
                ("unsupported", Some(1), "glob in *.rs")
            ]
        );
    }

    #[test]
    fn other_unchecked_constructs_are_unsupported() {
        for (command, detail) in [
            ("FOO=1 cargo test", "assignment FOO"),
            ("FOO=1", "no command word in FOO=1"),
            ("diff <(sort a) b", "process substitution"),
            (
                "cat <<EOF\n$(curl x)\nEOF",
                "heredoc with an unquoted delimiter",
            ),
            ("echo x > $f", "variable in redirect $f"),
        ] {
            assert_eq!(
                floors(&analyse_here(command))
                    .into_iter()
                    .map(|f| f.2)
                    .collect::<Vec<_>>(),
                vec![detail],
                "{command}"
            );
        }
    }
}

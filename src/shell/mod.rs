//! Shell command analysis: splits a command into segments (simple commands) and records the
//! constructs that need a closer look. Pure: no policy and no I/O.

mod words;
mod wrappers;

use brush_parser::ast::{
    Assignment, AssignmentName, AssignmentValue, Command, CommandPrefixOrSuffixItem,
    CompoundCommand, CompoundList, CompoundListItem, IoFileRedirectKind, IoFileRedirectTarget,
    IoRedirect, Pipeline, ProcessSubstitutionKind, RedirectList, SeparatorOperator, SimpleCommand,
    SubshellCommand, Word,
};
use brush_parser::{Parser, ParserOptions};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use words::NotStatic;

/// Substitutions nested deeper than this are unsupported
const MAX_DEPTH: usize = 16;

/// Builtins whose `NAME=value` arguments set variables
const DECLARATIONS: [&str; 5] = ["export", "declare", "typeset", "local", "readonly"];

/// The `[shell]` config
#[derive(Debug, Default)]
pub struct Settings {
    /// Variable names that may be assigned without asking
    pub safe_env: Vec<Regex>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Command,
    /// Only assignments (`FOO=1`): needs no command rule
    AssignmentOnly,
}

/// One simple command, as rules see it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    #[serde(skip)]
    pub kind: SegmentKind,
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

impl Segment {
    /// Neutral segments need no command rule, and rules never see them
    pub fn is_neutral(&self) -> bool {
        match self.kind {
            SegmentKind::Command => false,
            SegmentKind::AssignmentOnly => true,
        }
    }
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
    Expansion,
    DynamicCommand,
    EnvAssign,
    Substitution,
    Heredoc,
    Pipe,
    Background,
    Subshell,
    RedirectRead,
    RedirectWrite,
}

impl ConstructKind {
    pub fn name(self) -> &'static str {
        match self {
            ConstructKind::ParseError => "parse_error",
            ConstructKind::Unsupported => "unsupported",
            ConstructKind::Expansion => "expansion",
            ConstructKind::DynamicCommand => "dynamic_command",
            ConstructKind::EnvAssign => "env_assign",
            ConstructKind::Substitution => "substitution",
            ConstructKind::Heredoc => "heredoc",
            ConstructKind::Pipe => "pipe",
            ConstructKind::Background => "background",
            ConstructKind::Subshell => "subshell",
            ConstructKind::RedirectRead => "redirect_read",
            ConstructKind::RedirectWrite => "redirect_write",
        }
    }

    /// Floors always force at least an `ask`
    pub fn is_floor(self) -> bool {
        match self {
            ConstructKind::ParseError
            | ConstructKind::Unsupported
            | ConstructKind::Expansion
            | ConstructKind::DynamicCommand
            | ConstructKind::EnvAssign => true,
            ConstructKind::Substitution
            | ConstructKind::Heredoc
            | ConstructKind::Pipe
            | ConstructKind::Background
            | ConstructKind::Subshell
            | ConstructKind::RedirectRead
            | ConstructKind::RedirectWrite => false,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ConstructKind::ParseError => "the command could not be parsed",
            ConstructKind::Unsupported => "shell syntax that tool-gate-hook can't check",
            ConstructKind::Expansion => "a value only the shell can work out",
            ConstructKind::DynamicCommand => "a command name only the shell can work out",
            ConstructKind::EnvAssign => "a variable not listed in [shell] safe_env",
            ConstructKind::Substitution => "a command substitution",
            ConstructKind::Heredoc => "a heredoc or here-string",
            ConstructKind::Pipe => "a pipe",
            ConstructKind::Background => "a background command",
            ConstructKind::Subshell => "a subshell",
            ConstructKind::RedirectRead => "a read from a file",
            ConstructKind::RedirectWrite => "a write to a file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Construct {
    pub kind: ConstructKind,
    /// Index into `Analysis::segments`
    pub segment: Option<usize>,
    pub detail: Option<String>,
    /// The file, for redirects
    pub target: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// In textual order, except that a command comes before the commands substituted into it
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
                target: None,
            }],
        }
    }
}

pub fn analyse(command: &str, home: &Path, settings: &Settings) -> Analysis {
    let options = ParserOptions::default();
    let mut walker = Walker {
        home,
        settings,
        options: &options,
        depth: 0,
        analysis: Analysis::default(),
    };
    walker.program(command, None);
    walker.analysis
}

/// A command found inside a word or redirect, walked once its segment has been added
enum Pending<'a> {
    Substitution(String),
    Process(&'a SubshellCommand),
}

/// Where a word appears, which decides the floor and how it is described
#[derive(Clone, Copy)]
enum Role {
    Command,
    Arg,
    Redirect,
    HereString,
}

struct Walker<'a> {
    home: &'a Path,
    settings: &'a Settings,
    options: &'a ParserOptions,
    /// How many substitutions deep the walk is
    depth: usize,
    analysis: Analysis,
}

impl Walker<'_> {
    /// `segment` is the one a parse error is reported against
    fn program(&mut self, command: &str, segment: Option<usize>) {
        match Parser::new(Cursor::new(command), self.options).parse_program() {
            Ok(program) => {
                for list in &program.complete_commands {
                    self.list(list, &[]);
                }
            }
            Err(e) => self.floor(ConstructKind::ParseError, segment, e.to_string()),
        }
    }

    /// `redirects` are inherited from enclosing groups, and apply to every command inside
    fn list(&mut self, list: &CompoundList, redirects: &[Redirect]) {
        for CompoundListItem(and_or, separator) in &list.0 {
            let walk = |walker: &mut Self| {
                for (_, pipeline) in and_or.iter() {
                    walker.pipeline(pipeline, redirects);
                }
            };
            match separator {
                SeparatorOperator::Async => self.enclosing(ConstructKind::Background, walk),
                SeparatorOperator::Sequence => walk(self),
            }
        }
    }

    fn pipeline(&mut self, pipeline: &Pipeline, redirects: &[Redirect]) {
        if pipeline.seq.len() > 1 {
            self.enclosing(ConstructKind::Pipe, |walker| {
                walker.commands(pipeline, redirects);
            });
        } else {
            self.commands(pipeline, redirects);
        }
    }

    fn commands(&mut self, pipeline: &Pipeline, redirects: &[Redirect]) {
        let wrappers: &[&str] = if pipeline.timed.is_some() {
            &["time"]
        } else {
            &[]
        };
        for command in &pipeline.seq {
            match command {
                Command::Simple(simple) => self.simple(simple, redirects, wrappers),
                Command::Compound(CompoundCommand::Subshell(group), group_redirects) => {
                    self.enclosing(ConstructKind::Subshell, |walker| {
                        walker.group(&group.list, group_redirects.as_ref(), redirects);
                    });
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
                    self.floor(ConstructKind::Unsupported, None, kind);
                }
                Command::Function(_) => {
                    self.floor(ConstructKind::Unsupported, None, "function definition");
                }
                Command::ExtendedTest(..) => {
                    self.floor(ConstructKind::Unsupported, None, "[[ … ]] test");
                }
            }
        }
    }

    /// Records `kind` ahead of whatever `walk` finds, against the first segment inside
    fn enclosing(&mut self, kind: ConstructKind, walk: impl FnOnce(&mut Self)) {
        let position = self.analysis.constructs.len();
        let first = self.analysis.segments.len();
        walk(self);
        let construct = Construct {
            kind,
            segment: (self.analysis.segments.len() > first).then_some(first),
            detail: None,
            target: None,
        };
        self.analysis.constructs.insert(position, construct);
    }

    fn group(&mut self, list: &CompoundList, own: Option<&RedirectList>, inherited: &[Redirect]) {
        let mut redirects = inherited.to_vec();
        let mut pending = vec![];
        for redirect in own.iter().flat_map(|own| own.0.iter()) {
            if let Some(redirect) = self.redirect(redirect, None, &mut pending) {
                redirects.push(redirect);
            }
        }
        self.walk_pending(pending, None);
        self.list(list, &redirects);
    }

    fn simple(&mut self, simple: &SimpleCommand, inherited: &[Redirect], wrappers: &[&str]) {
        let prefix = simple.prefix.iter().flat_map(|prefix| prefix.0.iter());
        let assigns = prefix
            .clone()
            .any(|item| matches!(item, CommandPrefixOrSuffixItem::AssignmentWord(..)));
        let segment =
            (simple.word_or_name.is_some() || assigns).then_some(self.analysis.segments.len());
        let mut pending = vec![];
        let name = simple
            .word_or_name
            .as_ref()
            .map(|word| self.word(word, segment, Role::Command, &mut pending));
        let declares = name
            .as_deref()
            .is_some_and(|name| DECLARATIONS.contains(&name));
        let mut args = vec![];
        let mut env = BTreeMap::new();
        let mut redirects = inherited.to_vec();
        let suffix = simple.suffix.iter().flat_map(|suffix| suffix.0.iter());
        let items = prefix
            .map(|item| (true, item))
            .chain(suffix.map(|item| (false, item)));
        for (in_prefix, item) in items {
            match item {
                CommandPrefixOrSuffixItem::Word(word) => {
                    args.push(self.word(word, segment, Role::Arg, &mut pending));
                }
                CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    if let Some(redirect) = self.redirect(redirect, segment, &mut pending) {
                        redirects.push(redirect);
                    }
                }
                // brush-parser reads any `NAME=value` arg as an assignment (`make CC=gcc`)
                CommandPrefixOrSuffixItem::AssignmentWord(assignment, word) if in_prefix => {
                    if let Some((name, value)) =
                        self.assignment(assignment, word, segment, &mut pending)
                    {
                        env.insert(name, value);
                    }
                }
                CommandPrefixOrSuffixItem::AssignmentWord(assignment, word) => {
                    if declares {
                        self.assigned_name(assignment, word, segment);
                    }
                    args.push(self.word(word, segment, Role::Arg, &mut pending));
                }
                CommandPrefixOrSuffixItem::ProcessSubstitution(kind, subshell) => {
                    let text = process_substitution_text(kind, subshell);
                    self.floor(
                        ConstructKind::Expansion,
                        segment,
                        format!("process substitution in {text}"),
                    );
                    args.push(text);
                    pending.push(Pending::Process(subshell));
                }
            }
        }
        let mut segment_value = Segment {
            kind: SegmentKind::Command,
            text: String::new(),
            name: String::new(),
            args: vec![],
            env,
            redirects,
            wrappers: wrappers.iter().map(|w| (*w).to_owned()).collect(),
            source: simple.to_string(),
        };
        match name {
            Some(name) => {
                self.unwrap(&mut segment_value, name, args, segment);
                self.analysis.segments.push(segment_value);
            }
            None if assigns => {
                segment_value.kind = SegmentKind::AssignmentOnly;
                self.analysis.segments.push(segment_value);
            }
            None => self.floor(
                ConstructKind::Unsupported,
                None,
                format!("no command word in {simple}"),
            ),
        }
        self.walk_pending(pending, segment);
    }

    /// Fills in the command the wrappers (if any) run, and its `text`
    fn unwrap(
        &mut self,
        into: &mut Segment,
        name: String,
        args: Vec<String>,
        segment: Option<usize>,
    ) {
        let unwrapped = wrappers::unwrap(&name, &args).unwrap_or_else(|why| {
            self.floor(ConstructKind::Unsupported, segment, why);
            wrappers::Unwrapped {
                name,
                args,
                ..wrappers::Unwrapped::default()
            }
        });
        for (name, value) in unwrapped.env {
            self.check_env_name(&name, segment);
            into.env.insert(name, value);
        }
        into.wrappers.extend(unwrapped.wrappers);
        into.text = std::iter::once(&unwrapped.name)
            .chain(&unwrapped.args)
            .map(|word| words::quote(word))
            .collect::<Vec<_>>()
            .join(" ");
        into.name = unwrapped.name;
        into.args = unwrapped.args;
    }

    /// A prefix assignment's name and static value (or raw value, recording why it isn't static)
    fn assignment<'w>(
        &mut self,
        assignment: &'w Assignment,
        word: &Word,
        segment: Option<usize>,
        pending: &mut Vec<Pending<'w>>,
    ) -> Option<(String, String)> {
        let name = self.assigned_name(assignment, word, segment);
        match &assignment.value {
            AssignmentValue::Scalar(value) => {
                let value = self.value(&value.value, &word.value, segment, Role::Arg, pending);
                Some((name?.to_owned(), value))
            }
            AssignmentValue::Array(items) => {
                for (_, item) in items {
                    let info = words::analyse(&item.value, self.home, self.options);
                    pending.extend(info.substitutions.into_iter().map(Pending::Substitution));
                }
                None
            }
        }
    }

    /// Checks the name against `safe_env`; `None` for array assignments, which are unsupported
    fn assigned_name<'n>(
        &mut self,
        assignment: &'n Assignment,
        word: &Word,
        segment: Option<usize>,
    ) -> Option<&'n str> {
        match (&assignment.name, &assignment.value) {
            (AssignmentName::VariableName(name), AssignmentValue::Scalar(_)) => {
                self.check_env_name(name, segment);
                Some(name)
            }
            (AssignmentName::ArrayElementName(..), _) | (_, AssignmentValue::Array(_)) => {
                self.floor(
                    ConstructKind::Unsupported,
                    segment,
                    format!("array assignment {}", word.value),
                );
                None
            }
        }
    }

    fn check_env_name(&mut self, name: &str, segment: Option<usize>) {
        if !self
            .settings
            .safe_env
            .iter()
            .any(|safe| safe.is_match(name))
        {
            self.floor(
                ConstructKind::EnvAssign,
                segment,
                format!("assignment {name}"),
            );
        }
    }

    fn walk_pending(&mut self, pending: Vec<Pending<'_>>, segment: Option<usize>) {
        for item in pending {
            if self.depth >= MAX_DEPTH {
                self.floor(
                    ConstructKind::Unsupported,
                    segment,
                    format!("substitutions nested more than {MAX_DEPTH} deep"),
                );
                continue;
            }
            self.depth += 1;
            match item {
                Pending::Substitution(command) => {
                    self.record(
                        ConstructKind::Substitution,
                        segment,
                        Some(command.clone()),
                        None,
                    );
                    self.program(&command, segment);
                }
                Pending::Process(subshell) => {
                    let detail = subshell.list.to_string();
                    self.record(ConstructKind::Substitution, segment, Some(detail), None);
                    self.list(&subshell.list, &[]);
                }
            }
            self.depth -= 1;
        }
    }

    /// The word's static value; otherwise the raw word, recording why it isn't static
    fn word<'w>(
        &mut self,
        word: &Word,
        segment: Option<usize>,
        role: Role,
        pending: &mut Vec<Pending<'w>>,
    ) -> String {
        self.value(&word.value, &word.value, segment, role, pending)
    }

    /// The static value of `raw`; otherwise `raw`, recording why it isn't static, as part of
    /// `shown`
    fn value<'w>(
        &mut self,
        raw: &str,
        shown: &str,
        segment: Option<usize>,
        role: Role,
        pending: &mut Vec<Pending<'w>>,
    ) -> String {
        let info = words::analyse(raw, self.home, self.options);
        pending.extend(info.substitutions.into_iter().map(Pending::Substitution));
        info.value.unwrap_or_else(|why| {
            let place = match role {
                Role::Command | Role::Arg => "",
                Role::Redirect => "redirect ",
                Role::HereString => "here-string ",
            };
            let kind = match (why, role) {
                (NotStatic::Unsupported(_), _) => ConstructKind::Unsupported,
                (NotStatic::Expansion(_), Role::Command) => ConstructKind::DynamicCommand,
                (NotStatic::Expansion(_), _) => ConstructKind::Expansion,
            };
            self.floor(kind, segment, format!("{} in {place}{shown}", why.why()));
            raw.to_owned()
        })
    }

    fn redirect<'r>(
        &mut self,
        redirect: &'r IoRedirect,
        segment: Option<usize>,
        pending: &mut Vec<Pending<'r>>,
    ) -> Option<Redirect> {
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
                let (target, is_file) = match target {
                    IoFileRedirectTarget::Filename(word) => {
                        (self.word(word, segment, Role::Redirect, pending), true)
                    }
                    IoFileRedirectTarget::Duplicate(word) => {
                        let target = self.word(word, segment, Role::Redirect, pending);
                        let is_file = !is_fd(&target);
                        (target, is_file)
                    }
                    IoFileRedirectTarget::Fd(fd) => (fd.to_string(), false),
                    IoFileRedirectTarget::ProcessSubstitution(kind, subshell) => {
                        let text = process_substitution_text(kind, subshell);
                        self.floor(
                            ConstructKind::Expansion,
                            segment,
                            format!("process substitution in redirect {text}"),
                        );
                        pending.push(Pending::Process(subshell));
                        (text, false)
                    }
                };
                if is_file {
                    self.file_redirect(kind, &target, segment);
                }
                Some(Redirect {
                    op,
                    fd: *fd,
                    target,
                })
            }
            IoRedirect::HereDocument(fd, doc) => {
                let raw = &doc.here_end.value;
                let delimiter = words::analyse(raw, self.home, self.options)
                    .value
                    .unwrap_or_else(|_| raw.clone());
                self.record(
                    ConstructKind::Heredoc,
                    segment,
                    Some(delimiter.clone()),
                    None,
                );
                if doc.requires_expansion {
                    let info = words::analyse_heredoc(&doc.doc.value, self.options);
                    pending.extend(info.substitutions.into_iter().map(Pending::Substitution));
                    if let Err(why) = info.value {
                        let kind = match why {
                            NotStatic::Expansion(_) => ConstructKind::Expansion,
                            NotStatic::Unsupported(_) => ConstructKind::Unsupported,
                        };
                        self.floor(kind, segment, format!("{} in heredoc body", why.why()));
                    }
                }
                Some(Redirect {
                    op: if doc.remove_tabs { "<<-" } else { "<<" },
                    fd: *fd,
                    target: delimiter,
                })
            }
            IoRedirect::HereString(fd, word) => {
                self.record(ConstructKind::Heredoc, segment, None, None);
                Some(Redirect {
                    op: "<<<",
                    fd: *fd,
                    target: self.word(word, segment, Role::HereString, pending),
                })
            }
            IoRedirect::OutputAndError(word, append) => {
                let target = self.word(word, segment, Role::Redirect, pending);
                self.file_redirect(&IoFileRedirectKind::Write, &target, segment);
                Some(Redirect {
                    op: if *append { "&>>" } else { "&>" },
                    fd: None,
                    target,
                })
            }
        }
    }

    fn file_redirect(&mut self, kind: &IoFileRedirectKind, target: &str, segment: Option<usize>) {
        let construct = match kind {
            IoFileRedirectKind::Read => ConstructKind::RedirectRead,
            _ if target == "/dev/null" => return,
            IoFileRedirectKind::Write
            | IoFileRedirectKind::Append
            | IoFileRedirectKind::ReadAndWrite
            | IoFileRedirectKind::Clobber
            | IoFileRedirectKind::DuplicateOutput => ConstructKind::RedirectWrite,
            // `<&word` with a non-fd word is an error in bash
            IoFileRedirectKind::DuplicateInput => return,
        };
        self.record(construct, segment, None, Some(target.to_owned()));
    }

    fn floor(&mut self, kind: ConstructKind, segment: Option<usize>, detail: impl Into<String>) {
        self.record(kind, segment, Some(detail.into()), None);
    }

    fn record(
        &mut self,
        kind: ConstructKind,
        segment: Option<usize>,
        detail: Option<String>,
        target: Option<String>,
    ) {
        self.analysis.constructs.push(Construct {
            kind,
            segment,
            detail,
            target,
        });
    }
}

/// `1`, `-` and `3-` duplicate or close a descriptor; anything else in `>&word` is a file
fn is_fd(target: &str) -> bool {
    let digits = target.strip_suffix('-').unwrap_or(target);
    target == "-" || (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

fn process_substitution_text(kind: &ProcessSubstitutionKind, subshell: &SubshellCommand) -> String {
    let op = match kind {
        ProcessSubstitutionKind::Read => '<',
        ProcessSubstitutionKind::Write => '>',
    };
    format!("{op}({})", subshell.list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn analyse_here(command: &str) -> Analysis {
        analyse(command, Path::new("/home/me"), &Settings::default())
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
        assert_eq!(floors(&analysis), vec![]);
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
                kind: SegmentKind::Command,
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
        assert_eq!(floors(&analysis), vec![]);
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

    /// Non-floor constructs: name, segment, and detail or target
    fn structure(analysis: &Analysis) -> Vec<(&'static str, Option<usize>, &str)> {
        analysis
            .constructs
            .iter()
            .filter(|c| !c.kind.is_floor())
            .map(|c| {
                let about = c.target.as_deref().or(c.detail.as_deref());
                (c.kind.name(), c.segment, about.unwrap_or(""))
            })
            .collect()
    }

    #[test]
    fn non_static_words_are_expansions_or_dynamic_commands() {
        let analysis = analyse_here("echo $HOME && ls *.rs && $CMD x");

        assert_eq!(
            texts(&analysis),
            vec!["echo '$HOME'", "ls '*.rs'", "'$CMD' x"]
        );
        assert_eq!(
            floors(&analysis),
            vec![
                ("expansion", Some(0), "variable in $HOME"),
                ("expansion", Some(1), "glob in *.rs"),
                ("dynamic_command", Some(2), "variable in $CMD"),
            ]
        );
    }

    #[test]
    fn substituted_commands_follow_the_command_they_are_in() {
        let analysis = analyse_here("echo $(curl x) `date` && ls");

        assert_eq!(
            texts(&analysis),
            vec!["echo '$(curl x)' '`date`'", "curl x", "date", "ls"]
        );
        assert_eq!(
            structure(&analysis),
            vec![
                ("substitution", Some(0), "curl x"),
                ("substitution", Some(0), "date")
            ]
        );
    }

    #[test]
    fn process_substitutions_are_walked() {
        let analysis = analyse_here("diff <(sort a) b > >(tee log)");

        assert_eq!(
            texts(&analysis),
            vec!["diff '<(sort a)' b", "sort a", "tee log"]
        );
        assert_eq!(
            floors(&analysis),
            vec![
                ("expansion", Some(0), "process substitution in <(sort a)"),
                (
                    "expansion",
                    Some(0),
                    "process substitution in redirect >(tee log)"
                ),
            ]
        );
    }

    #[test]
    fn unquoted_heredoc_bodies_are_checked_and_walked() {
        let analysis = analyse_here("cat <<EOF\nhi $(curl x)\nEOF");

        assert_eq!(texts(&analysis), vec!["cat", "curl x"]);
        assert_eq!(
            floors(&analysis),
            vec![("expansion", Some(0), "command substitution in heredoc body")]
        );
        assert_eq!(
            structure(&analysis),
            vec![
                ("heredoc", Some(0), "EOF"),
                ("substitution", Some(0), "curl x")
            ]
        );
    }

    #[test]
    fn substitutions_nest_up_to_a_limit() {
        let nested = |depth: usize| format!("{}ls{}", "echo $(".repeat(depth), ")".repeat(depth));
        let deep_enough = analyse_here(&nested(16));
        let too_deep = analyse_here(&nested(17));

        assert_eq!(deep_enough.segments.len(), 17);
        assert!(!floors(&deep_enough).iter().any(|f| f.0 == "unsupported"));
        assert_eq!(
            floors(&too_deep).last(),
            Some(&(
                "unsupported",
                Some(16),
                "substitutions nested more than 16 deep"
            ))
        );
    }

    #[test]
    fn structure_is_recorded() {
        for (command, expected) in [
            (
                "cat < in | grep x > out 2>&1",
                vec![
                    ("pipe", Some(0), ""),
                    ("redirect_read", Some(0), "in"),
                    ("redirect_write", Some(1), "out"),
                ],
            ),
            (
                "ls >> a; ls &> b; ls >& c; ls <> d; ls >| e",
                vec![
                    ("redirect_write", Some(0), "a"),
                    ("redirect_write", Some(1), "b"),
                    ("redirect_write", Some(2), "c"),
                    ("redirect_write", Some(3), "d"),
                    ("redirect_write", Some(4), "e"),
                ],
            ),
            ("ls > /dev/null 2>&1 &> /dev/null 3>&-", vec![]),
            ("cargo test & ls", vec![("background", Some(0), "")]),
            (
                "(cd x && ls) > out",
                vec![("subshell", Some(0), ""), ("redirect_write", None, "out")],
            ),
            (
                "cat <<< hi <<'EOF'\nx\nEOF",
                vec![("heredoc", Some(0), ""), ("heredoc", Some(0), "EOF")],
            ),
        ] {
            assert_eq!(structure(&analyse_here(command)), expected, "{command}");
        }
    }

    fn analyse_with_safe_env(command: &str) -> Analysis {
        let settings = Settings {
            safe_env: vec![Regex::new("^RUST_LOG$").unwrap()],
        };
        analyse(command, Path::new("/home/me"), &settings)
    }

    #[test]
    fn prefix_assignments_go_to_env_and_unsafe_names_are_floors() {
        let analysis = analyse_with_safe_env("RUST_LOG=debug FOO='a b' cargo test");

        assert_eq!(texts(&analysis), vec!["cargo test"]);
        assert_eq!(
            analysis.segments[0].env,
            BTreeMap::from([
                ("FOO".to_owned(), "a b".to_owned()),
                ("RUST_LOG".to_owned(), "debug".to_owned())
            ])
        );
        assert_eq!(
            floors(&analysis),
            vec![("env_assign", Some(0), "assignment FOO")]
        );
    }

    #[test]
    fn assignment_values_are_classified_and_walked() {
        let analysis = analyse_with_safe_env("RUST_LOG=$x cargo test; RUST_LOG=$(curl x)");

        assert_eq!(texts(&analysis), vec!["cargo test", "", "curl x"]);
        assert_eq!(analysis.segments[0].env["RUST_LOG"], "$x");
        assert_eq!(
            floors(&analysis),
            vec![
                ("expansion", Some(0), "variable in RUST_LOG=$x"),
                (
                    "expansion",
                    Some(1),
                    "command substitution in RUST_LOG=$(curl x)"
                ),
            ]
        );
    }

    #[test]
    fn assignment_only_commands_are_neutral_segments() {
        let analysis = analyse_with_safe_env("RUST_LOG=1 > out; > out");

        assert_eq!(analysis.segments.len(), 1);
        assert!(analysis.segments[0].is_neutral());
        assert_eq!(analysis.segments[0].redirects[0].target, "out");
        assert_eq!(
            floors(&analysis),
            vec![("unsupported", None, "no command word in > out")]
        );
    }

    #[test]
    fn declaration_builtins_check_their_assignment_names() {
        for (command, expected) in [
            ("export RUST_LOG=1 FOO=2 BAR", vec!["assignment FOO"]),
            ("declare -x FOO=1", vec!["assignment FOO"]),
            (
                "typeset FOO=1; local FOO=1; readonly FOO=1",
                vec!["assignment FOO"; 3],
            ),
            ("make CC=gcc", vec![]),
            ("export A=(x y)", vec!["array assignment A=(x y)"]),
        ] {
            let analysis = analyse_with_safe_env(command);
            let details: Vec<&str> = floors(&analysis).iter().map(|f| f.2).collect();
            assert_eq!(details, expected, "{command}");
        }
        // The value is still an arg, so `text` shows it
        assert_eq!(texts(&analyse_here("export FOO=1")), vec!["export FOO=1"]);
    }

    #[test]
    fn wrappers_are_unwrapped() {
        let analysis =
            analyse_with_safe_env("time env RUST_LOG=debug FOO=1 timeout -s KILL 60 cargo test");

        assert_eq!(analysis.segments[0].text, "cargo test");
        assert_eq!(
            analysis.segments[0].wrappers,
            vec!["time", "env", "timeout"]
        );
        assert_eq!(analysis.segments[0].env.len(), 2);
        assert_eq!(
            floors(&analysis),
            vec![("env_assign", Some(0), "assignment FOO")]
        );
    }

    #[test]
    fn unknown_wrapper_options_leave_the_command_wrapped() {
        let analysis = analyse_here("timeout --bogus 60 cargo test");

        assert_eq!(analysis.segments[0].name, "timeout");
        assert_eq!(analysis.segments[0].wrappers, Vec::<String>::new());
        assert_eq!(
            floors(&analysis),
            vec![("unsupported", Some(0), "timeout option --bogus")]
        );
    }

    #[test]
    fn other_unchecked_constructs_are_unsupported() {
        let unicode = format!("$'{}u00e9'", '\\');
        for (command, detail) in [
            ("A[1]=x ls".to_owned(), "array assignment A[1]=x".to_owned()),
            (
                format!("echo {unicode}"),
                format!("unsupported $'…' escape in {unicode}"),
            ),
        ] {
            assert_eq!(
                floors(&analyse_here(&command)),
                vec![("unsupported", Some(0), detail.as_ref())],
                "{command}"
            );
        }
    }
}

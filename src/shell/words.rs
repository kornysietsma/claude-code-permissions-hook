//! Turning a raw shell word into the value the shell would pass, when that is knowable statically

use brush_parser::ParserOptions;
use brush_parser::word::{self, TildeExpr, WordPiece, WordPieceWithSource};
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

/// Why a word's value can't be known without running the shell
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotStatic {
    /// The shell would expand it; says which kind of expansion
    Expansion(&'static str),
    /// Something we don't decode
    Unsupported(&'static str),
}

impl NotStatic {
    pub fn why(self) -> &'static str {
        match self {
            NotStatic::Expansion(why) | NotStatic::Unsupported(why) => why,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct WordInfo {
    pub value: Result<String, NotStatic>,
    /// The raw commands inside `$( )` and backticks, anywhere in the word
    pub substitutions: Vec<String>,
}

pub fn analyse(raw: &str, home: &Path, options: &ParserOptions) -> WordInfo {
    match word::parse(raw, options) {
        Ok(pieces) => WordInfo {
            value: static_value(&pieces, home),
            substitutions: substitutions(&pieces),
        },
        Err(_) => unparseable(),
    }
}

/// A heredoc body with an unquoted delimiter, where only `$`, backticks and `\` are special
pub fn analyse_heredoc(raw: &str, options: &ParserOptions) -> WordInfo {
    let Ok(pieces) = word::parse_heredoc(raw, options) else {
        return unparseable();
    };
    let expansion = pieces
        .iter()
        .find_map(|WordPieceWithSource { piece, .. }| match piece {
            WordPiece::Text(text) if text.contains('$') => Some(NotStatic::Expansion("variable")),
            WordPiece::Text(_) | WordPiece::EscapeSequence(_) => None,
            other => Some(describe(other)),
        });
    WordInfo {
        value: expansion.map_or_else(|| Ok(raw.to_owned()), Err),
        substitutions: substitutions(&pieces),
    }
}

fn unparseable() -> WordInfo {
    WordInfo {
        value: Err(NotStatic::Unsupported("unparseable word")),
        substitutions: vec![],
    }
}

fn substitutions(pieces: &[WordPieceWithSource]) -> Vec<String> {
    pieces
        .iter()
        .flat_map(|WordPieceWithSource { piece, .. }| match piece {
            WordPiece::CommandSubstitution(command)
            | WordPiece::BackquotedCommandSubstitution(command) => vec![command.clone()],
            WordPiece::DoubleQuotedSequence(inner)
            | WordPiece::GettextDoubleQuotedSequence(inner) => substitutions(inner),
            WordPiece::Text(_)
            | WordPiece::SingleQuotedText(_)
            | WordPiece::AnsiCQuotedText(_)
            | WordPiece::TildeExpansion(_)
            | WordPiece::ParameterExpansion(_)
            | WordPiece::EscapeSequence(_)
            | WordPiece::ArithmeticExpression(_) => vec![],
        })
        .collect()
}

fn static_value(pieces: &[WordPieceWithSource], home: &Path) -> Result<String, NotStatic> {
    let mut value = String::new();
    // Unquoted text as written, with each quoted or expanded piece as `_`: where globs and
    // brace expansions can still happen
    let mut unquoted = String::new();
    for (i, WordPieceWithSource { piece, .. }) in pieces.iter().enumerate() {
        match piece {
            WordPiece::Text(text) => {
                value.push_str(text);
                unquoted.push_str(text);
            }
            WordPiece::SingleQuotedText(text) => {
                value.push_str(text);
                unquoted.push('_');
            }
            WordPiece::AnsiCQuotedText(text) => {
                value.push_str(
                    &decode_ansi_c(text)
                        .ok_or(NotStatic::Unsupported("unsupported $'…' escape"))?,
                );
                unquoted.push('_');
            }
            WordPiece::EscapeSequence(escape) => {
                value.push_str(unescape(escape));
                unquoted.push('_');
            }
            WordPiece::DoubleQuotedSequence(inner) => {
                for WordPieceWithSource { piece, .. } in inner {
                    match piece {
                        WordPiece::Text(text) => value.push_str(text),
                        WordPiece::EscapeSequence(escape) => value.push_str(unescape(escape)),
                        _ => return Err(describe(piece)),
                    }
                }
                unquoted.push('_');
            }
            WordPiece::TildeExpansion(TildeExpr::Home) if i == 0 => {
                value.push_str(&home.to_string_lossy());
                unquoted.push('_');
            }
            other => return Err(describe(other)),
        }
    }
    match unquoted_expansion(&unquoted) {
        Some(why) => Err(NotStatic::Expansion(why)),
        None => Ok(value),
    }
}

fn describe(piece: &WordPiece) -> NotStatic {
    NotStatic::Expansion(match piece {
        WordPiece::ParameterExpansion(_) => "variable",
        WordPiece::CommandSubstitution(_) | WordPiece::BackquotedCommandSubstitution(_) => {
            "command substitution"
        }
        WordPiece::ArithmeticExpression(_) => "arithmetic expansion",
        WordPiece::TildeExpansion(_) => "tilde expansion",
        WordPiece::GettextDoubleQuotedSequence(_) => "$\"…\" string",
        WordPiece::Text(_)
        | WordPiece::SingleQuotedText(_)
        | WordPiece::AnsiCQuotedText(_)
        | WordPiece::DoubleQuotedSequence(_)
        | WordPiece::EscapeSequence(_) => "expansion",
    })
}

/// Expansions that brush-parser leaves inside unquoted text
fn unquoted_expansion(unquoted: &str) -> Option<&'static str> {
    static GLOB_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[.*\]").unwrap());
    static EXTGLOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[+@!]\(").unwrap());
    static BRACES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{[^{}]*(,|\.\.)[^{}]*\}").unwrap());

    if unquoted.contains('$') {
        // brush reads zsh's `${(f)x}` as the text `$` + `{(f)x}`
        Some("variable")
    } else if unquoted.contains(['*', '?']) || GLOB_CLASS.is_match(unquoted) {
        Some("glob")
    } else if EXTGLOB.is_match(unquoted) {
        Some("extended glob")
    } else if BRACES.is_match(unquoted) {
        Some("brace expansion")
    } else if unquoted
        .strip_prefix('=')
        .is_some_and(|name| name.contains(|c| !"=-+".contains(c)))
    {
        // zsh replaces `=name` with the path of command `name`, or fails if there's none. No
        // command is called `===` or `-`, so separators like `====` only ever fail.
        Some("zsh =command expansion")
    } else {
        None
    }
}

/// `\x` outside single quotes is `x`
fn unescape(escape: &str) -> &str {
    escape.strip_prefix('\\').unwrap_or(escape)
}

/// The escapes of bash's `$'…'`, limited to ASCII results; `None` for anything else
fn decode_ansi_c(text: &str) -> Option<String> {
    let mut decoded = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            decoded.push(c);
            continue;
        }
        let escaped = match chars.next()? {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'a' => '\x07',
            'b' => '\x08',
            'e' | 'E' => '\x1b',
            'f' => '\x0c',
            'v' => '\x0b',
            c @ ('\\' | '\'' | '"' | '?') => c,
            'x' => {
                let digits: String = take_while(&mut chars, 2, |c| c.is_ascii_hexdigit());
                ascii(u32::from_str_radix(&digits, 16).ok()?)?
            }
            first @ '0'..='7' => {
                let rest: String = take_while(&mut chars, 2, |c| ('0'..='7').contains(&c));
                ascii(u32::from_str_radix(&format!("{first}{rest}"), 8).ok()?)?
            }
            _ => return None,
        };
        decoded.push(escaped);
    }
    Some(decoded)
}

fn take_while(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    max: usize,
    accept: impl Fn(char) -> bool,
) -> String {
    let mut taken = String::new();
    while taken.len() < max
        && let Some(&c) = chars.peek()
        && accept(c)
    {
        taken.push(c);
        chars.next();
    }
    taken
}

fn ascii(code: u32) -> Option<char> {
    char::from_u32(code).filter(char::is_ascii)
}

/// Quotes a value for display so it reads back as one word: plain when safe, else single-quoted
pub fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn word(raw: &str) -> WordInfo {
        analyse(raw, Path::new("/home/me"), &ParserOptions::default())
    }

    fn value(raw: &str) -> Result<String, String> {
        word(raw).value.map_err(|why| why.why().to_owned())
    }

    fn heredoc(raw: &str) -> WordInfo {
        analyse_heredoc(raw, &ParserOptions::default())
    }

    #[test]
    fn substitutions_are_found_anywhere_in_a_word() {
        assert_eq!(
            word(r#"a$(curl x)"`date`"$x"#).substitutions,
            vec!["curl x", "date"]
        );
        assert_eq!(word("'$(curl x)'").substitutions, Vec::<String>::new());
    }

    #[test]
    fn undecodable_words_are_unsupported_rather_than_expansions() {
        assert_eq!(
            word(r"$'\xff'").value,
            Err(NotStatic::Unsupported("unsupported $'…' escape"))
        );
        assert_eq!(word("$x").value, Err(NotStatic::Expansion("variable")));
    }

    #[test]
    fn heredoc_bodies_expand_only_dollars_and_backticks() {
        let literal = "it's \"quoted\" *.rs {a,b} ~/x\n";
        assert_eq!(heredoc(literal).value, Ok(literal.to_owned()));
        assert_eq!(
            heredoc("hi $USER\n").value,
            Err(NotStatic::Expansion("variable"))
        );
        assert_eq!(
            heredoc("${(f)x}\n").value,
            Err(NotStatic::Expansion("variable"))
        );
        let substituted = heredoc("a $(curl x)\nb `date`\n");
        assert_eq!(
            substituted.value,
            Err(NotStatic::Expansion("command substitution"))
        );
        assert_eq!(substituted.substitutions, vec!["curl x", "date"]);
    }

    #[test]
    fn quoting_is_removed() {
        assert_eq!(value(r#"a"b"'c'"#), Ok("abc".into()));
        assert_eq!(value(r#""x\"y""#), Ok(r#"x"y"#.into()));
        assert_eq!(value(r#""a\b""#), Ok(r"a\b".into()));
        assert_eq!(value(r"a\ b"), Ok("a b".into()));
        assert_eq!(value(r"\$z"), Ok("$z".into()));
        assert_eq!(value("'$HOME'"), Ok("$HOME".into()));
        assert_eq!(value("''"), Ok(String::new()));
    }

    #[test]
    fn ansi_c_strings_are_decoded() {
        assert_eq!(value(r"$'tab\there'"), Ok("tab\there".into()));
        assert_eq!(value(r"$'\x41\101\''"), Ok("AA'".into()));
        let unicode = format!("$'{}u00e9'", '\\');
        assert_eq!(value(&unicode), Err("unsupported $'…' escape".into()));
        assert_eq!(value(r"$'\xff'"), Err("unsupported $'…' escape".into()));
    }

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        assert_eq!(value("~/x"), Ok("/home/me/x".into()));
        assert_eq!(value("~"), Ok("/home/me".into()));
        assert_eq!(value("a~b"), Ok("a~b".into()));
        assert_eq!(value("~root/x"), Err("tilde expansion".into()));
    }

    #[test]
    fn expansions_are_not_static() {
        assert_eq!(value("$x"), Err("variable".into()));
        assert_eq!(value(r#""$x""#), Err("variable".into()));
        assert_eq!(value("${(f)x}"), Err("variable".into()));
        assert_eq!(value("$(date)"), Err("command substitution".into()));
        assert_eq!(value("`date`"), Err("command substitution".into()));
        assert_eq!(value("$((1+2))"), Err("arithmetic expansion".into()));
    }

    #[test]
    fn unquoted_globs_braces_and_equals_are_not_static() {
        assert_eq!(value("*.rs"), Err("glob".into()));
        assert_eq!(value("a?"), Err("glob".into()));
        assert_eq!(value("[ab]"), Err("glob".into()));
        assert_eq!(value("@(a|b)"), Err("extended glob".into()));
        assert_eq!(value("{a,b}"), Err("brace expansion".into()));
        assert_eq!(value("x{1..3}"), Err("brace expansion".into()));
        assert_eq!(value("=python3"), Err("zsh =command expansion".into()));
        assert_eq!(value("=a=b"), Err("zsh =command expansion".into()));
        assert_eq!(value("=-x"), Err("zsh =command expansion".into()));
        assert_eq!(value("='ls'"), Err("zsh =command expansion".into()));
    }

    #[test]
    fn quoted_or_harmless_special_characters_are_static() {
        assert_eq!(value("'*.rs'"), Ok("*.rs".into()));
        assert_eq!(value(r"\*"), Ok("*".into()));
        assert_eq!(value("'{a,b}'"), Ok("{a,b}".into()));
        assert_eq!(value("{}"), Ok("{}".into()));
        assert_eq!(value("HEAD@{1}"), Ok("HEAD@{1}".into()));
        assert_eq!(value("["), Ok("[".into()));
        assert_eq!(value("]"), Ok("]".into()));
        assert_eq!(value("="), Ok("=".into()));
        assert_eq!(value("===="), Ok("====".into()));
        assert_eq!(value("=-=-"), Ok("=-=-".into()));
        assert_eq!(value("=+"), Ok("=+".into()));
        assert_eq!(value("a=b"), Ok("a=b".into()));
        assert_eq!(value("a#b"), Ok("a#b".into()));
    }

    #[test]
    fn quote_leaves_plain_words_alone_and_single_quotes_the_rest() {
        assert_eq!(quote("cargo"), "cargo");
        assert_eq!(quote("--out=a/b.txt"), "--out=a/b.txt");
        assert_eq!(quote("my file"), "'my file'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a;b"), "'a;b'");
    }
}

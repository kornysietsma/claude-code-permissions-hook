//! Step 1 spike: what does brush-parser give us? Run with
//! `cargo test --test shell_spike -- --ignored --nocapture`

use brush_parser::ast::{
    Command, CommandPrefixOrSuffixItem, CompoundCommand, CompoundList, IoRedirect, Pipeline,
    SimpleCommand,
};
use brush_parser::word;
use brush_parser::{Parser, ParserOptions};
use std::fs;
use std::io::Cursor;

const NASTY: &[&str] = &[
    r#"cargo build 2>&1 && cargo test | tee out.txt"#,
    r#"echo a"b"'c' $'tab\there' "x\"y" \$z"#,
    r#"git commit -F - <<'EOF'
pip install x
EOF"#,
    "cat <<EOF\n$(curl evil)\nEOF",
    "cat <<<\"$HOME\"",
    "FOO=1 BAR=$x cargo test",
    "PATH=./evil; cargo test",
    "echo $(git rev-parse HEAD) `date` <(sort a)",
    "ls *.rs {a,b} ~/x ~root/y =python3",
    "ls @(a|b) *(e:'rm x':)",
    "time cargo test",
    "! false",
    "coproc foo",
    "[[ -f x ]] && echo y",
    "if true; then echo; fi",
    "( cd sub && make ) &",
    "{ echo a; echo b; } > out",
    "cmd &> all.log; cmd 3>&- 2>>err",
    "echo 'unterminated",
    "echo $((1+2)) ${x:-y}",
    "foo # a comment\nbar",
    "echo a\\\nb",
    "${(f)x}",
    "print -r -- $x",
    r#"echo "a\b" "\|" 'x\y' a\ b \n $'\x41\u00e9\n' "a#b" a#b"#,
];

fn options() -> ParserOptions {
    ParserOptions::default()
}

fn show_word(label: &str, raw: &str) {
    match word::parse(raw, &options()) {
        Ok(pieces) => {
            let pieces: Vec<String> = pieces.iter().map(|p| format!("{:?}", p.piece)).collect();
            println!("    {label} {raw:?} => {}", pieces.join(" + "));
        }
        Err(e) => println!("    {label} {raw:?} => WORD ERROR {e}"),
    }
}

fn walk_list(list: &CompoundList, depth: usize) {
    for item in &list.0 {
        println!("{}list item sep={:?}", "  ".repeat(depth), item.1);
        for (op, pipeline) in item.0.iter() {
            let _ = op;
            walk_pipeline(pipeline, depth + 1);
        }
    }
}

fn walk_pipeline(pipeline: &Pipeline, depth: usize) {
    println!(
        "{}pipeline timed={:?} bang={} len={}",
        "  ".repeat(depth),
        pipeline.timed.is_some(),
        pipeline.bang,
        pipeline.seq.len()
    );
    for command in &pipeline.seq {
        match command {
            Command::Simple(simple) => walk_simple(simple, depth + 1),
            Command::Compound(CompoundCommand::Subshell(s), redirects) => {
                println!("{}subshell redirects={redirects:?}", "  ".repeat(depth + 1));
                walk_list(&s.list, depth + 2);
            }
            Command::Compound(CompoundCommand::BraceGroup(b), redirects) => {
                println!(
                    "{}brace group redirects={redirects:?}",
                    "  ".repeat(depth + 1)
                );
                walk_list(&b.list, depth + 2);
            }
            other => println!("{}UNSUPPORTED {other:?}", "  ".repeat(depth + 1)),
        }
    }
}

fn walk_simple(simple: &SimpleCommand, depth: usize) {
    println!("{}simple command", "  ".repeat(depth));
    let items = simple
        .prefix
        .iter()
        .flat_map(|p| p.0.iter())
        .map(|i| ("prefix", i))
        .chain(simple.word_or_name.iter().map(|_| ("name", &NAME_MARKER)))
        .chain(
            simple
                .suffix
                .iter()
                .flat_map(|s| s.0.iter())
                .map(|i| ("suffix", i)),
        );
    for (label, item) in items {
        if label == "name" {
            let name = simple.word_or_name.as_ref().unwrap();
            show_word("name", &name.value);
            println!(
                "      loc={:?}",
                name.loc.as_ref().map(|l| (l.start.index, l.end.index))
            );
            continue;
        }
        match item {
            CommandPrefixOrSuffixItem::Word(w) => show_word(label, &w.value),
            CommandPrefixOrSuffixItem::AssignmentWord(a, w) => {
                println!(
                    "    {label} assignment {:?} = {:?} (word {:?})",
                    a.name, a.value, w.value
                )
            }
            CommandPrefixOrSuffixItem::IoRedirect(r) => show_redirect(r),
            CommandPrefixOrSuffixItem::ProcessSubstitution(kind, s) => {
                println!("    {label} process substitution {kind:?}");
                walk_list(&s.list, depth + 2);
            }
        }
    }
}

// Placeholder so the name can sit in the item sequence
static NAME_MARKER: CommandPrefixOrSuffixItem =
    CommandPrefixOrSuffixItem::Word(brush_parser::ast::Word {
        value: String::new(),
        loc: None,
    });

fn show_redirect(r: &IoRedirect) {
    match r {
        IoRedirect::HereDocument(fd, doc) => println!(
            "    heredoc fd={fd:?} requires_expansion={} end={:?} doc={:?}",
            doc.requires_expansion, doc.here_end.value, doc.doc.value
        ),
        other => println!("    redirect {other:?}"),
    }
}

fn analyse(command: &str) {
    println!("\n=== {command:?}");
    let mut parser = Parser::new(Cursor::new(command), &options());
    match parser.parse_program() {
        Ok(program) => {
            for complete in &program.complete_commands {
                walk_list(complete, 1);
            }
        }
        Err(e) => println!("  PARSE ERROR {e}"),
    }
}

#[test]
#[ignore]
fn spike_nasty_cases() {
    for command in NASTY {
        analyse(command);
    }
}

#[test]
#[ignore]
fn spike_local_corpus() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shell/corpus.local.jsonl"
    );
    let Ok(corpus) = fs::read_to_string(path) else {
        println!("no local corpus at {path}");
        return;
    };
    for line in corpus.lines() {
        let command: String = serde_json::from_str(line).unwrap();
        analyse(&command);
    }
}

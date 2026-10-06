//! Real commands: a curated, sanitised set with expected outcomes, and a summary of the local
//! corpus to spot gaps. Build the local corpus with `scripts/shell-corpus.sh AUDIT_LOG`, then run
//! `cargo test --test shell_corpus -- --ignored --nocapture`

use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use tool_gate_hook::shell::{Settings, analyse};

#[derive(Debug, Deserialize, PartialEq)]
struct Expected {
    command: String,
    /// Segment names, in order
    names: Vec<String>,
    /// The floors that fired, sorted
    floors: BTreeSet<String>,
}

#[test]
fn curated_corpus_is_analysed_as_expected() {
    let corpus = include_str!("fixtures/shell/corpus.jsonl");
    for line in corpus.lines() {
        let expected: Expected = serde_json::from_str(line).unwrap();
        let analysis = analyse(
            &expected.command,
            Path::new("/home/me"),
            Path::new("/tmp"),
            &Settings::default(),
        );
        let actual = Expected {
            command: expected.command.clone(),
            names: analysis.segments.iter().map(|s| s.name.clone()).collect(),
            floors: analysis
                .constructs
                .iter()
                .filter(|c| c.kind.is_floor())
                .map(|c| c.kind.name().to_owned())
                .collect(),
        };
        assert_eq!(actual, expected);
    }
}

#[test]
#[ignore]
fn summarise_local_corpus() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shell/corpus.local.jsonl"
    );
    let Ok(corpus) = fs::read_to_string(path) else {
        println!("no local corpus at {path}");
        return;
    };
    let mut floors: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut clean = 0;
    let mut total = 0;
    for line in corpus.lines() {
        let command: String = serde_json::from_str(line).unwrap();
        total += 1;
        let analysis = analyse(
            &command,
            Path::new("/home/me"),
            Path::new("/tmp"),
            &Settings::default(),
        );
        let mut found: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for construct in analysis.constructs.iter().filter(|c| c.kind.is_floor()) {
            found
                .entry(construct.kind.name())
                .or_default()
                .push(construct.detail.as_deref().unwrap_or(""));
        }
        if found.is_empty() {
            clean += 1;
        }
        let short: String = command.chars().take(100).collect();
        for (kind, details) in found {
            floors
                .entry(kind)
                .or_default()
                .push(format!("{short:?}\n      {}", details.join("; ")));
        }
    }
    println!("{total} commands, {clean} with no floor");
    for (kind, commands) in &floors {
        println!("\n{kind}: {}", commands.len());
        for command in commands {
            println!("  {command}");
        }
    }
}

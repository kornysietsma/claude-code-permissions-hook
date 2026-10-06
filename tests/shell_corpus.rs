//! Summarises how real commands are analysed, to spot gaps. Build the local corpus with
//! `scripts/shell-corpus.sh AUDIT_LOG`, then run
//! `cargo test --test shell_corpus -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tool_gate_hook::shell::analyse;

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
    let mut floors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut clean = 0;
    let mut total = 0;
    for line in corpus.lines() {
        let command: String = serde_json::from_str(line).unwrap();
        total += 1;
        let analysis = analyse(&command, Path::new("/home/me"));
        let mut kinds: Vec<_> = analysis
            .constructs
            .iter()
            .filter(|c| c.kind.is_floor())
            .map(|c| c.kind.name())
            .collect();
        kinds.dedup();
        if kinds.is_empty() {
            clean += 1;
        }
        for kind in kinds {
            floors
                .entry(kind.to_owned())
                .or_default()
                .push(command.clone());
        }
    }
    println!("{total} commands, {clean} with no floor");
    for (kind, commands) in &floors {
        println!("\n{kind}: {}", commands.len());
        for command in commands {
            let short: String = command.chars().take(120).collect();
            println!("  {short:?}");
        }
    }
}

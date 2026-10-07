use crate::agent::Agent;
use crate::config::Config;
use crate::policy::{ConstructRule, Decision, Policy, Rule};
use crate::shell::Segment;

#[derive(Debug, PartialEq, Eq)]
pub struct Validation {
    /// What the config contains, for stdout
    pub summary: String,
    /// Things that are legal but probably mistakes, for stderr
    pub warnings: Vec<String>,
}

pub fn validate(agent: Agent, config: &Config) -> Validation {
    let mut warnings = unknown_field_warnings(agent, &config.policy);
    warnings.extend(unknown_segment_field_warnings(&config.policy));
    Validation {
        summary: summary(config),
        warnings,
    }
}

fn summary(config: &Config) -> String {
    let policy = &config.policy;
    let safe_env = if policy.shell.safe_env.is_empty() {
        "none".to_owned()
    } else {
        let regexes: Vec<String> = policy
            .shell
            .safe_env
            .iter()
            .map(|regex| format!("'{}'", regex.as_str()))
            .collect();
        regexes.join(", ")
    };
    let patterns = if config.pattern_names.is_empty() {
        "none".to_owned()
    } else {
        config.pattern_names.join(", ")
    };
    let audit = match &config.audit {
        None => "off (no [audit] section)".to_owned(),
        Some(audit) => format!(
            "{} (level {}, max_value_len {})",
            audit.file.display(),
            audit.level.as_str(),
            audit.max_value_len
        ),
    };
    format!(
        "{}\n{}\n{}\nsafe_env: {safe_env}\npatterns: {patterns}\naudit: {audit}",
        counts(&policy.rules, "rules"),
        counts(&policy.command_rules, "command rules"),
        construct_rules(&policy.construct_rules),
    )
}

fn counts(rules: &[Rule], kind: &str) -> String {
    let count = |decision| rules.iter().filter(|r| r.decision == decision).count();
    format!(
        "{} {kind} (allow {}, ask {}, deny {})",
        rules.len(),
        count(Decision::Allow),
        count(Decision::Ask),
        count(Decision::Deny),
    )
}

/// e.g. `2 construct rules (ask on background; ask on redirect_write outside {cwd}, /tmp)`
fn construct_rules(rules: &[ConstructRule]) -> String {
    if rules.is_empty() {
        return "0 construct rules".to_owned();
    }
    let each: Vec<String> = rules
        .iter()
        .map(|rule| {
            let mut text = format!("{} on {}", rule.decision.as_str(), rule.construct.name());
            if !rule.outside.is_empty() {
                text.push_str(&format!(" outside {}", rule.outside.join(", ")));
            }
            text
        })
        .collect();
    format!("{} construct rules ({})", rules.len(), each.join("; "))
}

/// A cheap guard against copy-pasting rules between agents
fn unknown_field_warnings(agent: Agent, policy: &Policy) -> Vec<String> {
    let known = agent.top_level_keys();
    policy
        .rules
        .iter()
        .flat_map(|rule| {
            rule.fields
                .iter()
                .filter(|field| !known.contains(&first_component(&field.path)))
                .map(move |field| {
                    format!(
                        "{}: field \"{}\" does not start with a payload key known for {} ({})",
                        rule.label(),
                        field.path,
                        agent.name(),
                        known.join(", ")
                    )
                })
        })
        .collect()
}

/// Catches payload paths (`tool_input.command`) in command rules, which match segments
fn unknown_segment_field_warnings(policy: &Policy) -> Vec<String> {
    policy
        .command_rules
        .iter()
        .flat_map(|rule| {
            rule.fields
                .iter()
                .filter(|field| !Segment::FIELDS.contains(&first_component(&field.path)))
                .map(move |field| {
                    format!(
                        "{}: field \"{}\" does not start with a segment field ({})",
                        rule.label(),
                        field.path,
                        Segment::FIELDS.join(", ")
                    )
                })
        })
        .collect()
}

fn first_component(path: &str) -> &str {
    path.split('.').next().unwrap_or_default()
}

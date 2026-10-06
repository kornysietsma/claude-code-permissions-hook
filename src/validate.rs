use crate::agent::Agent;
use crate::config::Config;
use crate::policy::{Decision, Policy, Rule};

#[derive(Debug, PartialEq, Eq)]
pub struct Validation {
    /// What the config contains, for stdout
    pub summary: String,
    /// Things that are legal but probably mistakes, for stderr
    pub warnings: Vec<String>,
}

pub fn validate(agent: Agent, config: &Config) -> Validation {
    Validation {
        summary: summary(config),
        warnings: unknown_field_warnings(agent, &config.policy),
    }
}

fn summary(config: &Config) -> String {
    let counts = |rules: &[Rule], kind: &str| {
        let count = |decision| rules.iter().filter(|r| r.decision == decision).count();
        format!(
            "{} {kind} (allow {}, ask {}, deny {})",
            rules.len(),
            count(Decision::Allow),
            count(Decision::Ask),
            count(Decision::Deny),
        )
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
        "{}\n{}\npatterns: {patterns}\naudit: {audit}",
        counts(&config.policy.rules, "rules"),
        counts(&config.policy.command_rules, "command rules"),
    )
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
                .filter(|field| {
                    let first = field.path.split('.').next().unwrap_or_default();
                    !known.contains(&first)
                })
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

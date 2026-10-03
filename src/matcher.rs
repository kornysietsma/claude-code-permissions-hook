use crate::config::Rule;
use crate::hook_io::HookInput;
use log::{debug, trace};

/// Checks rules against input, returning the reason if a rule matches.
pub fn check_rules(rules: &[Rule], input: &HookInput) -> Option<String> {
    trace!(
        "Checking {} rules for tool: {}",
        rules.len(),
        input.tool_name
    );

    for (idx, rule) in rules.iter().enumerate() {
        if rule.tool != input.tool_name {
            trace!("Rule {} skipped - tool mismatch", idx);
            continue;
        }

        trace!("Evaluating rule {} for tool: {}", idx, input.tool_name);
        if let Some(decision) = check_rule(rule, input) {
            debug!("Rule {} matched: {:?}", idx, decision);
            return Some(decision);
        }
    }
    trace!("No rules matched for tool: {}", input.tool_name);
    None
}

fn check_rule(rule: &Rule, input: &HookInput) -> Option<String> {
    match input.tool_name.as_str() {
        "Read" | "Write" | "Edit" | "Glob" => {
            if let Some(file_path) = input.extract_field("file_path")
                && check_field_with_exclude(
                    &file_path,
                    &rule.file_path_regex,
                    &rule.file_path_exclude_regex,
                )
            {
                return Some(format!(
                    "Matched rule for {} with file_path: {}",
                    input.tool_name, file_path
                ));
            }
        }
        "Bash" => {
            if let Some(command) = input.extract_field("command")
                && check_field_with_exclude(
                    &command,
                    &rule.command_regex,
                    &rule.command_exclude_regex,
                )
            {
                return Some(format!("Matched rule for Bash with command: {}", command));
            }
        }
        "Task" => {
            if let Some(subagent_type) = input.extract_field("subagent_type")
                && check_subagent_type(rule, &subagent_type)
            {
                return Some(format!(
                    "Matched rule for Task with subagent_type: {}",
                    subagent_type
                ));
            }
            if let Some(prompt) = input.extract_field("prompt")
                && check_field_with_exclude(&prompt, &rule.prompt_regex, &rule.prompt_exclude_regex)
            {
                return Some("Matched rule for Task with prompt pattern".to_string());
            }
        }
        _ => {}
    }

    None
}

fn check_field_with_exclude(
    value: &str,
    main_regex: &Option<regex::Regex>,
    exclude_regex: &Option<regex::Regex>,
) -> bool {
    if let Some(regex) = main_regex {
        if !regex.is_match(value) {
            trace!("Main regex didn't match value: {}", value);
            return false;
        }
        if let Some(exclude) = exclude_regex
            && exclude.is_match(value)
        {
            debug!(
                "Rule matched but EXCLUDED by exclude pattern. Value: {}",
                value
            );
            return false;
        }
        trace!("Field matched: {}", value);
        return true;
    }
    false
}

fn check_subagent_type(rule: &Rule, subagent_type: &str) -> bool {
    if let Some(ref expected_type) = rule.subagent_type {
        if expected_type != subagent_type {
            trace!(
                "Subagent type didn't match. Expected: {}, got: {}",
                expected_type, subagent_type
            );
            return false;
        }
        if let Some(ref exclude) = rule.subagent_type_exclude_regex
            && exclude.is_match(subagent_type)
        {
            debug!(
                "Subagent type matched but EXCLUDED by exclude pattern: {}",
                subagent_type
            );
            return false;
        }
        trace!("Subagent type matched: {}", subagent_type);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn test_check_field_with_exclude() {
        let main_regex = Some(Regex::new(r"^/home/.*").unwrap());
        let exclude_regex = Some(Regex::new(r"\.\.").unwrap());

        assert!(check_field_with_exclude(
            "/home/user/file.txt",
            &main_regex,
            &exclude_regex
        ));
        assert!(!check_field_with_exclude(
            "/home/user/../etc/passwd",
            &main_regex,
            &exclude_regex
        ));
        assert!(!check_field_with_exclude(
            "/etc/passwd",
            &main_regex,
            &exclude_regex
        ));
    }

    #[test]
    fn test_check_subagent_type() {
        let rule = Rule {
            tool: "Task".to_string(),
            file_path_regex: None,
            file_path_exclude_regex: None,
            command_regex: None,
            command_exclude_regex: None,
            subagent_type: Some("codebase-analyzer".to_string()),
            subagent_type_exclude_regex: None,
            prompt_regex: None,
            prompt_exclude_regex: None,
        };

        assert!(check_subagent_type(&rule, "codebase-analyzer"));
        assert!(!check_subagent_type(&rule, "other-agent"));
    }
}

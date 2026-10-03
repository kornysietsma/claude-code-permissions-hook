// Audit records are rebuilt in plan step 5.1; only the truncation helper survives until then

/// Recursively truncate string fields in a JSON value that exceed `max_len` characters.
pub fn truncate_json_strings(value: &serde_json::Value, max_len: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.chars().count() <= max_len {
                value.clone()
            } else {
                let truncated: String = s.chars().take(max_len).collect();
                serde_json::Value::String(format!("{}…", truncated))
            }
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| truncate_json_strings(v, max_len))
                .collect(),
        ),
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), truncate_json_strings(v, max_len)))
                .collect(),
        ),
        // Numbers, bools, null pass through unchanged
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn test_truncate_short_string_unchanged() {
        let input = json!("short string");
        let result = truncate_json_strings(&input, 100);
        assert_eq!(result, input);
    }

    #[test]
    fn test_truncate_long_string() {
        let long_string = "x".repeat(200);
        let input = json!(long_string);
        let result = truncate_json_strings(&input, 50);

        let truncated = result.as_str().unwrap();
        assert!(truncated.ends_with("…"));
        assert!(truncated.starts_with("xxxxxxxxxx"));
    }

    #[test]
    fn test_truncate_preserves_object_structure() {
        let long_content = "y".repeat(200);
        let input = json!({
            "file_path": "/short/path.rs",
            "content": long_content
        });
        let result = truncate_json_strings(&input, 50);

        // Structure preserved
        assert!(result.is_object());
        let obj = result.as_object().unwrap();

        // Short field unchanged
        assert_eq!(obj.get("file_path").unwrap(), "/short/path.rs");

        // Long field truncated
        let content = obj.get("content").unwrap().as_str().unwrap();
        assert!(content.ends_with("…"));
    }

    #[test]
    fn test_truncate_nested_structures() {
        let long_string = "z".repeat(100);
        let input = json!({
            "outer": {
                "inner": long_string.clone()
            },
            "array": ["short", long_string]
        });
        let result = truncate_json_strings(&input, 20);

        // Nested object string truncated
        let inner = result["outer"]["inner"].as_str().unwrap();
        assert!(inner.ends_with("…"));

        // Array elements handled
        assert_eq!(result["array"][0], "short");
        let arr_long = result["array"][1].as_str().unwrap();
        assert!(arr_long.ends_with("…"));
    }

    #[test]
    fn test_truncate_non_strings_unchanged() {
        let input = json!({
            "number": 42,
            "bool": true,
            "null": null
        });
        let result = truncate_json_strings(&input, 10);
        assert_eq!(result, input);
    }
}

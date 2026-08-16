//! A small JSON Schema: enough to fail a generate run when the model did not
//! say what the caller asked for. Not a schema catalogue and not $ref.

use aidb_core::{Error, Result};
use serde_json::Value;

pub fn parse_schema(json: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(json.trim())
        .map_err(|err| Error::usage(format!("generate schema is not JSON: {err}")))?;
    if !value.is_object() {
        return Err(Error::usage(
            "generate schema must be a JSON object (a JSON Schema)",
        ));
    }
    Ok(value)
}

/// Pull a JSON value out of model text: a bare JSON value, or a fenced block.
pub fn extract_json(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    if let Some(fenced) = fenced_json(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(fenced) {
            return Ok(value);
        }
    }
    if let Some(blob) = first_json_blob(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(blob) {
            return Ok(value);
        }
    }
    // A classify-style bare label is a JSON string once quoted.
    if !trimmed.is_empty()
        && !trimmed.starts_with(['{', '[', '"'])
        && !trimmed.contains('\n')
        && trimmed.len() < 64
    {
        return Ok(Value::String(trimmed.to_string()));
    }
    Err("model output is not JSON".to_string())
}

pub fn validate(schema: &Value, instance: &Value) -> Result<(), String> {
    if let Some(expected) = schema.get("const") {
        if instance != expected {
            return Err(format!("expected const {expected}, got {instance}"));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum") {
        if !options.iter().any(|option| option == instance) {
            return Err(format!("expected one of {options:?}, got {instance}"));
        }
    }
    if let Some(type_name) = schema.get("type").and_then(Value::as_str) {
        type_ok(type_name, instance)?;
        match type_name {
            "object" => validate_object(schema, instance)?,
            "array" => validate_array(schema, instance)?,
            "number" | "integer" => validate_number(schema, instance)?,
            _ => {}
        }
    } else if schema.get("properties").is_some() || schema.get("required").is_some() {
        type_ok("object", instance)?;
        validate_object(schema, instance)?;
    }
    Ok(())
}

fn type_ok(type_name: &str, instance: &Value) -> Result<(), String> {
    let ok = match type_name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "number" => instance.is_number(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        other => return Err(format!("unsupported schema type: {other}")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("expected {type_name}, got {instance}"))
    }
}

fn validate_object(schema: &Value, instance: &Value) -> Result<(), String> {
    let object = instance
        .as_object()
        .ok_or_else(|| format!("expected object, got {instance}"))?;
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required {
            let Some(name) = key.as_str() else {
                continue;
            };
            if !object.contains_key(name) {
                return Err(format!("missing property {name}"));
            }
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in properties {
            if let Some(value) = object.get(name) {
                validate(sub, value).map_err(|err| format!("{name}: {err}"))?;
            }
        }
    }
    Ok(())
}

fn validate_array(schema: &Value, instance: &Value) -> Result<(), String> {
    let items = instance
        .as_array()
        .ok_or_else(|| format!("expected array, got {instance}"))?;
    if let Some(item_schema) = schema.get("items") {
        for (i, item) in items.iter().enumerate() {
            validate(item_schema, item).map_err(|err| format!("items[{i}]: {err}"))?;
        }
    }
    Ok(())
}

fn validate_number(schema: &Value, instance: &Value) -> Result<(), String> {
    let number = instance
        .as_f64()
        .ok_or_else(|| format!("expected number, got {instance}"))?;
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
        if number < min {
            return Err(format!("{number} is below minimum {min}"));
        }
    }
    if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
        if number > max {
            return Err(format!("{number} is above maximum {max}"));
        }
    }
    Ok(())
}

fn fenced_json(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let after = after
        .strip_prefix("json")
        .or_else(|| after.strip_prefix("JSON"))
        .unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let end = after.find("```")?;
    Some(after[..end].trim())
}

fn first_json_blob(text: &str) -> Option<&str> {
    let start = text.find(['{', '['])?;
    let open = text.as_bytes()[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    for (i, byte) in text.as_bytes()[start..].iter().enumerate() {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(&text[start..start + i + 1]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_object_missing_a_required_field_is_rejected() {
        let schema = json!({"type":"object","required":["ticker"],"properties":{"ticker":{"type":"string"}}});
        assert!(validate(&schema, &json!({"ticker":"NVDA"})).is_ok());
        let err = validate(&schema, &json!({"name":"NVDA"})).unwrap_err();
        assert!(err.contains("ticker"), "{err}");
    }

    #[test]
    fn an_enum_rejects_a_label_that_was_not_listed() {
        let schema = json!({"enum":["positive","negative"]});
        assert!(validate(&schema, &json!("positive")).is_ok());
        assert!(validate(&schema, &json!("sideways")).is_err());
    }

    #[test]
    fn a_bare_label_extracts_as_a_json_string() {
        assert_eq!(extract_json("negative").unwrap(), json!("negative"));
        assert_eq!(
            extract_json("```json\n{\"ticker\":\"NVDA\"}\n```").unwrap(),
            json!({"ticker":"NVDA"})
        );
    }

    #[test]
    fn a_schema_that_is_not_json_is_a_usage_error() {
        let err = parse_schema("not json").unwrap_err().to_string();
        assert!(err.contains("not JSON"), "{err}");
    }
}

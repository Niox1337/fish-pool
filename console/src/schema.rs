//! Plugin config handling against `_conf_schema.json`: default generation and validation.

use serde_json::{Map, Value};

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

fn field_type(def: &Value) -> &str {
    def.get("type").and_then(Value::as_str).unwrap_or("string")
}

fn type_default(def: &Value) -> Value {
    match field_type(def) {
        "int" => Value::from(0),
        "float" => Value::from(0.0),
        "bool" => Value::Bool(false),
        "list" | "template_list" => Value::Array(vec![]),
        "object" => defaults(def.get("items").unwrap_or(&Value::Null)),
        "dict" => Value::Object(Map::new()),
        _ => Value::String(String::new()),
    }
}

/// Builds the default config object for a schema.
pub fn defaults(schema: &Value) -> Value {
    let mut out = Map::new();
    if let Some(fields) = schema.as_object() {
        for (key, def) in fields {
            let v = match (field_type(def), def.get("default")) {
                ("object", Some(Value::Object(given))) => {
                    let mut base = defaults(def.get("items").unwrap_or(&Value::Null));
                    merge_missing(&mut base, &Value::Object(given.clone()), true);
                    base
                }
                (_, Some(d)) => d.clone(),
                _ => type_default(def),
            };
            out.insert(key.clone(), v);
        }
    }
    Value::Object(out)
}

/// Copies keys of `src` into `dst`. With `overwrite` existing keys are replaced, nested objects merge.
pub fn merge_missing(dst: &mut Value, src: &Value, overwrite: bool) {
    let (Some(d), Some(s)) = (dst.as_object_mut(), src.as_object()) else {
        return;
    };
    for (k, v) in s {
        match d.get_mut(k) {
            Some(existing) if existing.is_object() && v.is_object() => merge_missing(existing, v, overwrite),
            Some(existing) => {
                if overwrite {
                    *existing = v.clone();
                }
            }
            None => {
                d.insert(k.clone(), v.clone());
            }
        }
    }
}

fn check_field(path: &str, def: &Value, v: &Value) -> Result<(), String> {
    let ok = match field_type(def) {
        "string" | "text" => v.is_string(),
        "int" => v.as_i64().is_some() || v.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false),
        "float" => v.is_number(),
        "bool" => v.is_boolean(),
        "list" | "template_list" => v.is_array(),
        "object" | "dict" => v.is_object(),
        _ => true,
    };
    if !ok {
        return Err(format!("配置项 {path} 的类型应为 {}", field_type(def)));
    }
    if let (Some(options), Some(s)) = (def.get("options").and_then(Value::as_array), v.as_str()) {
        if !options.is_empty() && !options.iter().any(|o| o.as_str() == Some(s)) {
            return Err(format!("配置项 {path} 的取值不在可选范围内"));
        }
    }
    if let (Some(slider), Some(n)) = (def.get("slider"), v.as_f64()) {
        let min = slider.get("min").and_then(Value::as_f64);
        let max = slider.get("max").and_then(Value::as_f64);
        if min.map(|m| n < m).unwrap_or(false) || max.map(|m| n > m).unwrap_or(false) {
            return Err(format!("配置项 {path} 超出允许范围"));
        }
    }
    if field_type(def) == "object" {
        if let Some(items) = def.get("items") {
            validate_at(path, items, v)?;
        }
    }
    if let ("template_list", Some(entries), Some(templates)) = (field_type(def), v.as_array(), def.get("templates")) {
        for (index, entry) in entries.iter().enumerate() {
            if let Some(items) = entry
                .get("__template_key")
                .and_then(Value::as_str)
                .and_then(|key| templates.get(key))
                .and_then(|template| template.get("items"))
            {
                validate_at(&format!("{path}[{index}]"), items, entry)?;
            }
        }
    }
    Ok(())
}

fn validate_at(prefix: &str, schema: &Value, config: &Value) -> Result<(), String> {
    let Some(cfg) = config.as_object() else {
        return Err("配置必须是 JSON 对象".into());
    };
    let Some(fields) = schema.as_object() else {
        return Ok(());
    };
    for (k, v) in cfg {
        if let Some(def) = fields.get(k) {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            check_field(&path, def, v)?;
        }
    }
    Ok(())
}

/// Validates a config object. Keys missing from the schema are kept untouched.
pub fn validate(schema: Option<&Value>, config: &Value) -> Result<(), String> {
    if !config.is_object() {
        return Err("配置必须是 JSON 对象".into());
    }
    if serde_json::to_vec(config).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_CONFIG_BYTES {
        return Err("配置内容过大".into());
    }
    match schema {
        Some(s) => validate_at("", s, config),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "token": {"type": "string", "description": "令牌"},
            "count": {"type": "int", "default": 3, "slider": {"min": 1, "max": 10, "step": 1}},
            "ratio": {"type": "float"},
            "on": {"type": "bool", "default": true},
            "mode": {"type": "string", "options": ["a", "b"], "default": "a"},
            "tags": {"type": "list"},
            "nested": {"type": "object", "items": {"x": {"type": "int", "default": 5}, "y": {"type": "text"}}},
            "extra": {"type": "dict"},
            "tpl": {"type": "template_list"}
        })
    }

    #[test]
    fn builds_defaults() {
        let d = defaults(&schema());
        assert_eq!(
            d,
            json!({"token": "", "count": 3, "ratio": 0.0, "on": true, "mode": "a", "tags": [],
                   "nested": {"x": 5, "y": ""}, "extra": {}, "tpl": []})
        );
        assert_eq!(defaults(&Value::Null), json!({}));
    }

    #[test]
    fn validates_types_options_and_ranges() {
        let s = schema();
        assert!(validate(Some(&s), &defaults(&s)).is_ok());
        assert!(validate(Some(&s), &json!({"count": 4.0})).is_ok());
        assert!(validate(Some(&s), &json!({"count": "4"})).is_err());
        assert!(validate(Some(&s), &json!({"count": 11})).is_err());
        assert!(validate(Some(&s), &json!({"count": 2.5})).is_err());
        assert!(validate(Some(&s), &json!({"on": 1})).is_err());
        assert!(validate(Some(&s), &json!({"mode": "c"})).is_err());
        assert!(validate(Some(&s), &json!({"tags": "x"})).is_err());
        assert!(validate(Some(&s), &json!({"nested": {"x": "no"}})).is_err());
        assert!(validate(Some(&s), &json!({"nested": {"x": 1, "other": true}})).is_ok());
        assert!(validate(Some(&s), &json!({"unknown": [1, 2]})).is_ok());
        assert!(validate(Some(&s), &json!([1])).is_err());
        assert!(validate(None, &json!({"a": 1})).is_ok());
        let big = json!({"token": "x".repeat(MAX_CONFIG_BYTES)});
        assert!(validate(Some(&s), &big).is_err());
    }

    #[test]
    fn merges() {
        let mut base = json!({"a": 1, "n": {"x": 1, "y": 2}});
        merge_missing(&mut base, &json!({"a": 9, "b": 2, "n": {"y": 3, "z": 4}}), false);
        assert_eq!(base, json!({"a": 1, "b": 2, "n": {"x": 1, "y": 2, "z": 4}}));
        merge_missing(&mut base, &json!({"a": 9, "n": {"y": 3}}), true);
        assert_eq!(base, json!({"a": 9, "b": 2, "n": {"x": 1, "y": 3, "z": 4}}));
    }

    #[test]
    fn validates_known_template_items_and_preserves_legacy_entries() {
        let schema = json!({"rules": {"type": "template_list", "templates": {
            "rule": {"items": {"count": {"type": "int"}}}
        }}});
        assert!(validate(
            Some(&schema),
            &json!({"rules": [
                {"__template_key": "rule", "count": 2}, {"__template_key": "removed", "extra": true}
            ]})
        )
        .is_ok());
        let err = validate(
            Some(&schema),
            &json!({"rules": [{"__template_key": "rule", "count": "2"}]}),
        )
        .unwrap_err();
        assert!(err.contains("rules[0].count"));
    }
}

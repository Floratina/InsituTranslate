use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::domain::ProviderConfigIssue;
use crate::providers::registry::{ConfigField, ConfigFieldKind};

pub fn parse_default(field: &ConfigField) -> Result<Option<Value>, String> {
    field
        .default_json
        .map(|value| {
            serde_json::from_str(value).map_err(|error| {
                format!(
                    "Config field {} has invalid default JSON: {error}",
                    field.pointer
                )
            })
        })
        .transpose()
}

pub fn validate_fields(fields: &[ConfigField]) -> Result<(), String> {
    let mut pointers = HashSet::new();
    let mut paths = Vec::with_capacity(fields.len());
    for field in fields {
        let path = parse_object_pointer(field.pointer)?;
        if !pointers.insert(field.pointer) {
            return Err(format!(
                "Duplicate provider config field pointer: {}",
                field.pointer
            ));
        }
        if paths.iter().any(|existing: &Vec<String>| {
            path.starts_with(existing.as_slice()) || existing.starts_with(path.as_slice())
        }) {
            return Err(format!(
                "Overlapping provider config field pointer: {}",
                field.pointer
            ));
        }
        paths.push(path);

        match field.kind {
            ConfigFieldKind::Select => validate_select_options(field)?,
            _ if !field.options.is_empty() => {
                return Err(format!(
                    "Non-select config field {} cannot declare options",
                    field.pointer
                ));
            }
            _ => {}
        }
        if let Some(default) = parse_default(field)? {
            if let Some(message) = field_value_issue(field, &default) {
                return Err(format!(
                    "Config field {} has invalid default: {message}",
                    field.pointer
                ));
            }
        }
    }
    Ok(())
}

pub fn materialize_defaults(config: Value, fields: &[ConfigField]) -> Result<Value, String> {
    let Value::Object(mut object) = config else {
        return Err("Provider config must be a JSON object".into());
    };
    for field in fields {
        let path = parse_object_pointer(field.pointer)?;
        if pointer_value(&Value::Object(object.clone()), &path).is_none() {
            if let Some(default) = parse_default(field)? {
                insert_pointer(&mut object, &path, default)?;
            }
        }
    }
    Ok(Value::Object(object))
}

pub fn config_issues(config: &Value, fields: &[ConfigField]) -> Vec<ProviderConfigIssue> {
    let Value::Object(_) = config else {
        return vec![ProviderConfigIssue {
            pointer: String::new(),
            message: "Provider config must be a JSON object".into(),
        }];
    };
    fields
        .iter()
        .filter_map(|field| {
            let path = match parse_object_pointer(field.pointer) {
                Ok(path) => path,
                Err(message) => {
                    return Some(ProviderConfigIssue {
                        pointer: field.pointer.to_string(),
                        message,
                    });
                }
            };
            match pointer_value(config, &path) {
                None if field.required => Some(ProviderConfigIssue {
                    pointer: field.pointer.to_string(),
                    message: format!("{} is required", field.label),
                }),
                None => None,
                Some(value) => field_value_issue(field, value).map(|message| ProviderConfigIssue {
                    pointer: field.pointer.to_string(),
                    message,
                }),
            }
        })
        .collect()
}

pub fn validated_config(config: Value, fields: &[ConfigField]) -> Result<Value, String> {
    let config = materialize_defaults(config, fields)?;
    let issues = config_issues(&config, fields);
    if issues.is_empty() {
        Ok(config)
    } else {
        Err(format_issues(&issues))
    }
}

pub fn format_issues(issues: &[ProviderConfigIssue]) -> String {
    issues
        .iter()
        .map(|issue| {
            if issue.pointer.is_empty() {
                issue.message.clone()
            } else {
                format!("{}: {}", issue.pointer, issue.message)
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn validate_select_options(field: &ConfigField) -> Result<(), String> {
    if field.options.is_empty() {
        return Err(format!(
            "Select config field {} must declare options",
            field.pointer
        ));
    }
    let mut values = HashSet::new();
    for option in field.options {
        if option.value.is_empty() {
            return Err(format!(
                "Select config field {} contains an empty option value",
                field.pointer
            ));
        }
        if !values.insert(option.value) {
            return Err(format!(
                "Select config field {} contains duplicate option {}",
                field.pointer, option.value
            ));
        }
    }
    Ok(())
}

fn field_value_issue(field: &ConfigField, value: &Value) -> Option<String> {
    if value.is_null() {
        return Some(if field.required {
            format!("{} is required", field.label)
        } else {
            format!("{} cannot be null", field.label)
        });
    }
    match field.kind {
        ConfigFieldKind::Text => match value.as_str() {
            Some(text) if field.required && text.trim().is_empty() => {
                Some(format!("{} is required", field.label))
            }
            Some(_) => None,
            None => Some(format!("{} must be a string", field.label)),
        },
        ConfigFieldKind::Number => {
            (!value.is_number()).then(|| format!("{} must be a number", field.label))
        }
        ConfigFieldKind::Boolean => {
            (!value.is_boolean()).then(|| format!("{} must be a boolean", field.label))
        }
        ConfigFieldKind::Select => match value.as_str() {
            Some(selected) if field.options.iter().any(|option| option.value == selected) => None,
            Some(_) => Some(format!("{} must use a registered option", field.label)),
            None => Some(format!("{} must be a string", field.label)),
        },
    }
}

fn parse_object_pointer(pointer: &str) -> Result<Vec<String>, String> {
    if !pointer.starts_with('/') {
        return Err(format!(
            "Provider config pointer must start with '/': {pointer}"
        ));
    }
    let mut output = Vec::new();
    for raw in pointer[1..].split('/') {
        let mut decoded = String::new();
        let mut characters = raw.chars();
        while let Some(character) = characters.next() {
            if character != '~' {
                decoded.push(character);
                continue;
            }
            match characters.next() {
                Some('0') => decoded.push('~'),
                Some('1') => decoded.push('/'),
                _ => {
                    return Err(format!(
                        "Provider config pointer contains an invalid escape: {pointer}"
                    ));
                }
            }
        }
        output.push(decoded);
    }
    Ok(output)
}

fn pointer_value<'a>(config: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = config;
    for segment in path {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn insert_pointer(
    object: &mut Map<String, Value>,
    path: &[String],
    value: Value,
) -> Result<(), String> {
    let Some((leaf, parents)) = path.split_last() else {
        return Err("Provider config pointer cannot target the document root".into());
    };
    let mut current = object;
    for segment in parents {
        let child = current
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        current = child.as_object_mut().ok_or_else(|| {
            format!("Provider config default path crosses non-object value at {segment}")
        })?;
    }
    current.insert(leaf.clone(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::registry::{ConfigField, ConfigFieldKind, ConfigOption};
    use serde_json::json;

    static OPTIONS: &[ConfigOption] = &[
        ConfigOption {
            value: "fast",
            label: "Fast",
        },
        ConfigOption {
            value: "quality",
            label: "Quality",
        },
    ];

    static FIELDS: &[ConfigField] = &[
        ConfigField {
            pointer: "/nested/mode",
            label: "Mode",
            kind: ConfigFieldKind::Select,
            required: true,
            default_json: Some("\"fast\""),
            options: OPTIONS,
            help_text: None,
        },
        ConfigField {
            pointer: "/escaped~1key/value~0name",
            label: "Limit",
            kind: ConfigFieldKind::Number,
            required: false,
            default_json: Some("3"),
            options: &[],
            help_text: None,
        },
        ConfigField {
            pointer: "/empty//leaf",
            label: "Empty segment",
            kind: ConfigFieldKind::Boolean,
            required: false,
            default_json: Some("true"),
            options: &[],
            help_text: None,
        },
    ];

    #[test]
    fn materializes_nested_and_escaped_defaults_without_removing_unknown_keys() {
        validate_fields(FIELDS).expect("valid fields");
        let config = materialize_defaults(json!({"unknown": true}), FIELDS).expect("defaults");
        assert_eq!(config.pointer("/nested/mode"), Some(&json!("fast")));
        assert_eq!(
            config
                .get("escaped/key")
                .and_then(|value| value.get("value~name")),
            Some(&json!(3))
        );
        assert_eq!(config.pointer("/empty//leaf"), Some(&json!(true)));
        assert_eq!(config["unknown"], true);
    }

    #[test]
    fn reports_required_type_and_select_violations() {
        let missing = config_issues(&json!({}), FIELDS);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].pointer, "/nested/mode");

        let invalid = config_issues(
            &json!({"nested": {"mode": "other"}, "escaped/key": {"value~name": "3"}}),
            FIELDS,
        );
        assert_eq!(invalid.len(), 2);
    }

    #[test]
    fn rejects_invalid_and_overlapping_pointers() {
        let invalid = [ConfigField {
            pointer: "/bad~2escape",
            label: "Bad",
            kind: ConfigFieldKind::Text,
            required: false,
            default_json: None,
            options: &[],
            help_text: None,
        }];
        assert!(validate_fields(&invalid).is_err());

        let overlapping = [
            ConfigField {
                pointer: "/nested",
                label: "Nested",
                kind: ConfigFieldKind::Text,
                required: false,
                default_json: None,
                options: &[],
                help_text: None,
            },
            FIELDS[0],
        ];
        assert!(validate_fields(&overlapping).is_err());
    }
}

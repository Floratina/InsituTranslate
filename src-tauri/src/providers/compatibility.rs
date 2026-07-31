use serde::Serialize;
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityChange {
    pub parameter: String,
    pub previous_value: Value,
    pub replacement_value: Value,
}

#[derive(Debug, Clone)]
pub struct CompatibilityPatch {
    pub rule_id: &'static str,
    pub body: Value,
    pub changes: Vec<CompatibilityChange>,
}

pub struct CompatibilityContext<'a> {
    pub wire_family: &'a str,
    pub base_url: &'a str,
    pub model_id: &'a str,
}

pub fn patch_for_error(
    context: CompatibilityContext<'_>,
    status: u16,
    error_message: &str,
    body: &Value,
) -> Option<CompatibilityPatch> {
    if !matches!(status, 400 | 422) || context.wire_family != "openai-chat" {
        return None;
    }
    let host = hostname(context.base_url)?;
    let message = error_message.to_ascii_lowercase();
    if !parameter_rejected(&message) {
        return None;
    }

    if host_allowed(&host, &["api.openai.com", "api.xiaomimimo.com"])
        && model_allowed(context.model_id, &["gpt-5", "o1", "o3", "o4", "mimo-"])
        && mentions_parameter(&message, "max_tokens")
    {
        return rename_parameter(
            body,
            "max_tokens",
            "max_completion_tokens",
            "openai-chat-max-tokens-to-max-completion-v1",
        );
    }

    if host_allowed(
        &host,
        &[
            "api.deepseek.com",
            "dashscope.aliyuncs.com",
            "dashscope-intl.aliyuncs.com",
            "api.siliconflow.cn",
            "api.siliconflow.com",
            "qianfan.baidubce.com",
        ],
    ) && model_allowed(
        context.model_id,
        &["deepseek-", "qwen", "ernie-", "glm-", "kimi-"],
    ) && mentions_parameter(&message, "max_completion_tokens")
    {
        return rename_parameter(
            body,
            "max_completion_tokens",
            "max_tokens",
            "openai-chat-max-completion-to-max-tokens-v1",
        );
    }

    None
}

fn rename_parameter(
    body: &Value,
    source: &str,
    target: &str,
    rule_id: &'static str,
) -> Option<CompatibilityPatch> {
    let mut body = body.clone();
    let object = body.as_object_mut()?;
    if object.contains_key(target) {
        return None;
    }
    let previous_value = object.remove(source)?;
    object.insert(target.to_string(), previous_value.clone());
    Some(CompatibilityPatch {
        rule_id,
        body,
        changes: vec![CompatibilityChange {
            parameter: format!("{source} -> {target}"),
            previous_value: previous_value.clone(),
            replacement_value: previous_value,
        }],
    })
}

fn hostname(base_url: &str) -> Option<String> {
    Url::parse(base_url)
        .or_else(|_| Url::parse(&format!("https://{base_url}")))
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
}

fn host_allowed(host: &str, allowed: &[&str]) -> bool {
    allowed.contains(&host)
}

fn model_allowed(model_id: &str, prefixes: &[&str]) -> bool {
    let model_id = model_id
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    prefixes.iter().any(|prefix| model_id.starts_with(prefix))
}

fn parameter_rejected(message: &str) -> bool {
    [
        "unsupported",
        "not supported",
        "unknown parameter",
        "unrecognized",
        "unexpected",
        "invalid parameter",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn mentions_parameter(message: &str, parameter: &str) -> bool {
    message.contains(parameter)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn applies_only_an_allowlisted_host_model_and_error() {
        let patch = patch_for_error(
            CompatibilityContext {
                wire_family: "openai-chat",
                base_url: "https://api.xiaomimimo.com/v1",
                model_id: "mimo-v2-pro",
            },
            400,
            "Unsupported parameter: max_tokens",
            &json!({"model": "mimo-v2-pro", "max_tokens": 2048}),
        )
        .expect("allowlisted patch");

        assert_eq!(patch.rule_id, "openai-chat-max-tokens-to-max-completion-v1");
        assert_eq!(patch.body["max_completion_tokens"], 2048);
        assert!(patch.body.get("max_tokens").is_none());

        assert!(patch_for_error(
            CompatibilityContext {
                wire_family: "openai-chat",
                base_url: "https://example.test/v1",
                model_id: "mimo-v2-pro",
            },
            400,
            "Unsupported parameter: max_tokens",
            &json!({"max_tokens": 2048}),
        )
        .is_none());
    }

    #[test]
    fn never_overwrites_an_existing_target_parameter() {
        assert!(patch_for_error(
            CompatibilityContext {
                wire_family: "openai-chat",
                base_url: "https://api.openai.com/v1",
                model_id: "gpt-5",
            },
            400,
            "Unknown parameter max_tokens",
            &json!({"max_tokens": 100, "max_completion_tokens": 200}),
        )
        .is_none());
    }
}

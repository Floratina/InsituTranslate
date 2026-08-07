use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;

use crate::domain::ProviderRuntimeConfig;

use super::registry::{descriptor_for, AuthStrategy};
use super::{HeaderDirective, HeaderMode};

pub(super) async fn request_headers(
    client: &Client,
    config: &ProviderRuntimeConfig,
    directives: &[HeaderDirective],
) -> Result<HeaderMap, String> {
    let descriptor = descriptor_for(&config.protocol)?;
    let auth_strategy = descriptor.auth.strategy;
    let vertex_token = if auth_strategy == AuthStrategy::VertexServiceAccount {
        Some(crate::vertex_ai::access_token(client, config).await?)
    } else {
        None
    };
    build_headers(config, directives, vertex_token.as_deref())
}

fn build_headers(
    config: &ProviderRuntimeConfig,
    directives: &[HeaderDirective],
    vertex_token: Option<&str>,
) -> Result<HeaderMap, String> {
    let descriptor = descriptor_for(&config.protocol)?;
    let auth_strategy = descriptor.auth.strategy;
    let mut headers = HeaderMap::new();
    if let AuthStrategy::StaticHeader { header, scheme } = auth_strategy {
        if let Some(credential) = config
            .credential
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let name = HeaderName::from_bytes(header.as_bytes())
                .map_err(|error| format!("Invalid authentication header: {error}"))?;
            let value = match scheme {
                Some(scheme) => format!("{scheme} {credential}"),
                None => credential.to_string(),
            };
            headers.insert(
                name,
                HeaderValue::from_str(&value)
                    .map_err(|error| format!("Invalid authentication value: {error}"))?,
            );
        }
    }
    for (name, value) in &config.custom_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid custom header {name}: {error}"))?;
        if auth_strategy == AuthStrategy::VertexServiceAccount && header_name == AUTHORIZATION {
            continue;
        }
        headers.insert(
            header_name,
            HeaderValue::from_str(value)
                .map_err(|error| format!("Invalid custom header value for {name}: {error}"))?,
        );
    }
    if auth_strategy == AuthStrategy::VertexServiceAccount {
        let token = vertex_token.ok_or_else(|| {
            "Agent Platform OAuth token is required to construct request headers".to_string()
        })?;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|error| format!("Invalid Agent Platform token: {error}"))?,
        );
    }
    for directive in directives {
        let name = HeaderName::from_bytes(directive.name.as_bytes())
            .map_err(|error| format!("Invalid protocol header {}: {error}", directive.name))?;
        if directive.mode == HeaderMode::Replace || !headers.contains_key(&name) {
            let value = HeaderValue::from_str(&directive.value)
                .map_err(|error| format!("Invalid protocol header value: {error}"))?;
            headers.insert(name, value);
        }
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProtocolId;
    use serde_json::json;

    fn config(id: &str) -> ProviderRuntimeConfig {
        let descriptor = super::super::registry::descriptor_by_id(id)
            .expect("valid registry")
            .expect("registered protocol");
        ProviderRuntimeConfig {
            protocol: ProtocolId::registered(id),
            base_url: descriptor.default_base_url.into(),
            use_raw_base_url: false,
            config: json!({}),
            credential: None,
            custom_headers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn header_priority_preserves_custom_values_for_if_absent_directives() {
        let mut config = config("anthropic");
        config.credential = Some("credential-key".into());
        config.custom_headers = vec![
            ("x-api-key".into(), "custom-key".into()),
            ("anthropic-version".into(), "custom-version".into()),
        ];
        let headers = request_headers(
            &Client::new(),
            &config,
            &[HeaderDirective {
                name: "anthropic-version".into(),
                value: "2023-06-01".into(),
                mode: HeaderMode::IfAbsent,
            }],
        )
        .await
        .expect("headers");

        assert_eq!(headers["x-api-key"], "custom-key");
        assert_eq!(headers["anthropic-version"], "custom-version");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }

    #[test]
    fn header_order_preserves_custom_auth_and_forces_json_content_type_last() {
        let mut config = config("openai-chat");
        config.credential = Some("default-key".into());
        config.custom_headers = vec![
            ("Authorization".into(), "Custom credential".into()),
            ("Content-Type".into(), "text/plain".into()),
        ];
        let headers = build_headers(
            &config,
            &[HeaderDirective {
                name: "content-type".into(),
                value: "application/problem+json".into(),
                mode: HeaderMode::Replace,
            }],
            None,
        )
        .expect("headers");

        assert_eq!(headers[AUTHORIZATION], "Custom credential");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }

    #[test]
    fn vertex_oauth_has_priority_over_custom_authorization() {
        let mut config = config("vertex-ai");
        config.custom_headers = vec![
            ("Authorization".into(), "Custom credential".into()),
            ("X-Trace".into(), "trace-value".into()),
        ];
        let headers = build_headers(&config, &[], Some("oauth-token")).expect("vertex headers");

        assert_eq!(headers[AUTHORIZATION], "Bearer oauth-token");
        assert_eq!(headers["x-trace"], "trace-value");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }
}

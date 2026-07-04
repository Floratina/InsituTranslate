use regex::Regex;
use url::Url;

use crate::domain::{ProviderProtocol, ThinkingEffort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferredModelCapabilities {
    pub reasoning: bool,
    pub web: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FeatureId {
    OpenAiCacheControl,
    OpenAiReasoningObject,
    OpenAiThinkingObject,
    OpenAiReasoningEffort,
    OpenAiDeepSeekReasoningEffort,
    OpenAiEnableThinking,
    OpenAiThinkingBudget,
    OpenAiThinkingStrategy,
    OpenAiReasoningContent,
    OpenAiReasoningDetails,
    OpenAiReasoningField,
    OpenAiDisableReasoning,
    OpenAiClearThinking,
    OpenAiReasoningSplit,
    OpenAiOnlyMaxCompletionTokens,
    OpenAiOnlyMaxTokens,
    OpenAiTopK,
    OpenAiMaxInputTokens,
    GeminiThinkingLevel,
    AnthropicWebSearch,
    AnthropicInterleavedThinking,
}

fn hostname(base_url: &str) -> String {
    Url::parse(base_url)
        .or_else(|_| Url::parse(&format!("https://{base_url}")))
        .ok()
        .and_then(|url| url.host_str().map(str::to_lowercase))
        .unwrap_or_default()
}

fn provider_is(base_url: &str, patterns: &[&str]) -> bool {
    let host = hostname(base_url);
    patterns.iter().any(|pattern| host == *pattern)
}

fn model_starts(model_id: &str, patterns: &[&str]) -> bool {
    let model = model_id.to_lowercase();
    patterns.iter().any(|pattern| model.starts_with(pattern))
}

fn model_contains(model_id: &str, patterns: &[&str]) -> bool {
    let model = model_id.to_lowercase();
    patterns.iter().any(|pattern| model.contains(pattern))
}

fn model_name(model_id: &str) -> String {
    let model = model_id.trim().to_lowercase();
    model
        .rsplit("/models/")
        .next()
        .unwrap_or(&model)
        .trim_start_matches("models/")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn model_name_is(model_id: &str, ids: &[&str]) -> bool {
    let name = model_name(model_id);
    ids.iter().any(|id| name == *id)
}

fn model_name_starts(model_id: &str, patterns: &[&str]) -> bool {
    let name = model_name(model_id);
    patterns.iter().any(|pattern| name.starts_with(pattern))
}

fn baidu_model(base_url: &str, model_id: &str, ids: &[&str]) -> bool {
    provider_is(base_url, &["qianfan.baidubce.com"])
        && ids.iter().any(|id| model_id.eq_ignore_ascii_case(id))
}

pub fn is_openai_chat_search_model(model_id: &str) -> bool {
    model_name_is(
        model_id,
        &[
            "gpt-5-search-api",
            "gpt-4o-search-preview",
            "gpt-4o-mini-search-preview",
        ],
    )
}

fn gemini_google_search_model(model_id: &str) -> bool {
    model_name_starts(model_id, &["gemini-2.0", "gemini-2.5", "gemini-3"])
}

fn reasoning_model(protocol: ProviderProtocol, base_url: &str, model_id: &str) -> bool {
    match protocol {
        ProviderProtocol::OpenaiResponses => model_name_starts(
            model_id,
            &["gpt-5", "o1", "o3", "o4", "codex-mini", "gpt-oss"],
        ),
        ProviderProtocol::OpenaiChat => {
            model_name_starts(
                model_id,
                &["gpt-5", "o1", "o3", "o4", "codex-mini", "gpt-oss"],
            ) || is_feature_supported(FeatureId::OpenAiReasoningObject, base_url, model_id)
                || is_feature_supported(FeatureId::OpenAiThinkingObject, base_url, model_id)
                || is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id)
                || is_feature_supported(
                    FeatureId::OpenAiDeepSeekReasoningEffort,
                    base_url,
                    model_id,
                )
                || is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id)
        }
        ProviderProtocol::Anthropic => model_name_starts(
            model_id,
            &[
                "claude-opus-4",
                "claude-sonnet-4",
                "claude-haiku-4",
                "claude-3-7-sonnet",
            ],
        ),
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => {
            model_name_starts(model_id, &["gemini-2.5", "gemini-3", "gemma-4"])
        }
        ProviderProtocol::Ollama => {
            model_contains(model_id, &["deepseek-r1", "qwen3", "gpt-oss", "magistral"])
        }
    }
}

pub fn native_web_search_supported(
    protocol: ProviderProtocol,
    base_url: &str,
    model_id: &str,
) -> bool {
    match protocol {
        ProviderProtocol::OpenaiResponses => true,
        ProviderProtocol::OpenaiChat => is_openai_chat_search_model(model_id),
        ProviderProtocol::Anthropic => {
            is_feature_supported(FeatureId::AnthropicWebSearch, base_url, model_id)
        }
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => {
            gemini_google_search_model(model_id)
        }
        ProviderProtocol::Ollama => false,
    }
}

pub fn infer_model_capabilities(
    protocol: ProviderProtocol,
    base_url: &str,
    model_id: &str,
) -> InferredModelCapabilities {
    InferredModelCapabilities {
        reasoning: reasoning_model(protocol, base_url, model_id),
        web: native_web_search_supported(protocol, base_url, model_id),
    }
}

pub fn supported_thinking_efforts(
    protocol: ProviderProtocol,
    base_url: &str,
    model_id: &str,
    capability_reasoning: bool,
) -> Vec<ThinkingEffort> {
    if !capability_reasoning {
        return vec![ThinkingEffort::None];
    }

    let budget_efforts = vec![
        ThinkingEffort::None,
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
    ];
    let level_efforts = vec![
        ThinkingEffort::None,
        ThinkingEffort::Minimal,
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
    ];
    let openai_efforts = vec![
        ThinkingEffort::None,
        ThinkingEffort::Minimal,
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
        ThinkingEffort::Xhigh,
    ];

    match protocol {
        ProviderProtocol::OpenaiResponses => openai_efforts,
        ProviderProtocol::OpenaiChat => {
            if is_feature_supported(FeatureId::OpenAiDeepSeekReasoningEffort, base_url, model_id) {
                vec![
                    ThinkingEffort::None,
                    ThinkingEffort::High,
                    ThinkingEffort::Max,
                ]
            } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id) {
                budget_efforts
            } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id) {
                level_efforts
            } else {
                openai_efforts
            }
        }
        ProviderProtocol::Anthropic => budget_efforts,
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => {
            if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
                level_efforts
            } else {
                budget_efforts
            }
        }
        ProviderProtocol::Ollama => budget_efforts,
    }
}

pub fn is_feature_supported(feature: FeatureId, base_url: &str, model_id: &str) -> bool {
    match feature {
        FeatureId::OpenAiCacheControl => {
            provider_is(base_url, &["openrouter.ai"]) && model_contains(model_id, &["claude-"])
        }
        FeatureId::OpenAiReasoningObject => provider_is(base_url, &["openrouter.ai"]),
        FeatureId::OpenAiThinkingObject => {
            provider_is(
                base_url,
                &[
                    "ark.cn-beijing.volces.com",
                    "ark.ap-southeast.bytepluses.com",
                    "tokenhub.tencentmaas.com",
                    "tokenhub-intl.tencentmaas.com",
                    "api.lkeap.cloud.tencent.com",
                    "api.deepseek.com",
                    "api.xiaomimimo.com",
                    "open.bigmodel.cn",
                    "api.z.ai",
                    "api.moonshot.cn",
                    "api.moonshot.ai",
                ],
            ) || model_contains(model_id, &["deepseek-v4"])
                || baidu_model(
                    base_url,
                    model_id,
                    &[
                        "deepseek-v3.2",
                        "deepseek-v3.1",
                        "kimi-k2.5",
                        "glm-5",
                        "glm-4.7",
                    ],
                )
                || (provider_is(base_url, &["integrate.api.nvidia.com"])
                    && model_starts(model_id, &["z-ai/glm"]))
        }
        FeatureId::OpenAiReasoningEffort => {
            provider_is(
                base_url,
                &[
                    "ark.cn-beijing.volces.com",
                    "ark.ap-southeast.bytepluses.com",
                    "tokenhub.tencentmaas.com",
                    "tokenhub-intl.tencentmaas.com",
                    "api.lkeap.cloud.tencent.com",
                    "api.synthetic.new",
                ],
            ) || baidu_model(base_url, model_id, &["gpt-oss-120b", "gpt-oss-20b"])
        }
        FeatureId::OpenAiDeepSeekReasoningEffort => {
            model_contains(model_id, &["deepseek-v4"])
                || (provider_is(base_url, &["open.bigmodel.cn", "api.z.ai"])
                    && model_starts(model_id, &["glm-5"]))
        }
        FeatureId::OpenAiEnableThinking => {
            provider_is(
                base_url,
                &[
                    "dashscope.aliyuncs.com",
                    "dashscope-intl.aliyuncs.com",
                    "api-inference.modelscope.cn",
                    "api.siliconflow.cn",
                    "api.siliconflow.com",
                    "api.longcat.chat",
                    "wanqing.streamlakeapi.com",
                    "vanchin.streamlake.ai",
                ],
            ) || baidu_model(
                base_url,
                model_id,
                &[
                    "qwen3-235b-a22b",
                    "qwen3-30b-a3b",
                    "qwen3-32b",
                    "qwen3-14b",
                    "qwen3-8b",
                    "qwen3-4b",
                    "qwen3-1.7b",
                    "qwen3-0.6b",
                    "ernie-5.0-thinking-preview",
                ],
            )
        }
        FeatureId::OpenAiThinkingBudget => {
            provider_is(
                base_url,
                &[
                    "dashscope.aliyuncs.com",
                    "dashscope-intl.aliyuncs.com",
                    "api-inference.modelscope.cn",
                    "api.siliconflow.cn",
                    "api.siliconflow.com",
                    "api.longcat.chat",
                ],
            ) || baidu_model(
                base_url,
                model_id,
                &[
                    "ernie-5.0-thinking-preview",
                    "deepseek-v3.2-think",
                    "deepseek-r1-250528",
                    "qwen3-235b-a22b",
                    "qwen3-30b-a3b",
                ],
            )
        }
        FeatureId::OpenAiThinkingStrategy => provider_is(base_url, &["qianfan.baidubce.com"]),
        FeatureId::OpenAiReasoningContent => provider_is(
            base_url,
            &[
                "ark.cn-beijing.volces.com",
                "ark.ap-southeast.bytepluses.com",
                "tokenhub.tencentmaas.com",
                "tokenhub-intl.tencentmaas.com",
                "api.lkeap.cloud.tencent.com",
                "api.deepseek.com",
                "api.xiaomimimo.com",
                "open.bigmodel.cn",
                "api.z.ai",
                "api.moonshot.cn",
                "api.moonshot.ai",
                "api.kimi.com",
                "dashscope.aliyuncs.com",
                "dashscope-intl.aliyuncs.com",
                "api-inference.modelscope.cn",
                "api.siliconflow.cn",
                "api.siliconflow.com",
                "api.longcat.chat",
                "api.synthetic.new",
                "qianfan.baidubce.com",
                "integrate.api.nvidia.com",
            ],
        ),
        FeatureId::OpenAiReasoningDetails => provider_is(
            base_url,
            &["openrouter.ai", "api.minimaxi.com", "api.minimax.io"],
        ),
        FeatureId::OpenAiReasoningField => provider_is(
            base_url,
            &["api.cerebras.ai", "api.stepfun.com", "api.stepfun.ai"],
        ),
        FeatureId::OpenAiDisableReasoning => {
            provider_is(base_url, &["api.cerebras.ai"]) && model_starts(model_id, &["zai-glm-4.7"])
        }
        FeatureId::OpenAiClearThinking => {
            provider_is(base_url, &["open.bigmodel.cn", "api.z.ai"])
                || (provider_is(base_url, &["api.cerebras.ai", "integrate.api.nvidia.com"])
                    && model_contains(model_id, &["glm-4.7", "glm4.7"]))
        }
        FeatureId::OpenAiReasoningSplit => {
            provider_is(base_url, &["integrate.api.nvidia.com"])
                && model_starts(model_id, &["minimaxai/minimax-"])
        }
        FeatureId::OpenAiOnlyMaxCompletionTokens => {
            provider_is(
                base_url,
                &[
                    "api.cerebras.ai",
                    "opencode.ai",
                    "api.synthetic.new",
                    "api.moonshot.cn",
                    "api.moonshot.ai",
                    "api.kimi.com",
                ],
            ) || model_starts(
                model_id,
                &[
                    "gpt-5",
                    "o1",
                    "o3",
                    "o4-mini",
                    "codex-mini",
                    "gpt-oss",
                    "mimo-",
                ],
            )
        }
        FeatureId::OpenAiOnlyMaxTokens => provider_is(
            base_url,
            &[
                "ark.cn-beijing.volces.com",
                "ark.ap-southeast.bytepluses.com",
                "tokenhub.tencentmaas.com",
                "tokenhub-intl.tencentmaas.com",
                "api.lkeap.cloud.tencent.com",
                "router.huggingface.co",
                "qianfan.baidubce.com",
                "portal.qwen.ai",
                "api.siliconflow.cn",
                "api.siliconflow.com",
                "api.stepfun.com",
                "api.stepfun.ai",
            ],
        ),
        FeatureId::OpenAiTopK => provider_is(
            base_url,
            &[
                "dashscope.aliyuncs.com",
                "dashscope-intl.aliyuncs.com",
                "api-inference.modelscope.cn",
                "api.synthetic.new",
            ],
        ),
        FeatureId::OpenAiMaxInputTokens => provider_is(
            base_url,
            &[
                "dashscope.aliyuncs.com",
                "dashscope-intl.aliyuncs.com",
                "api-inference.modelscope.cn",
            ],
        ),
        FeatureId::GeminiThinkingLevel => model_starts(
            model_id,
            &[
                "gemini-3-",
                "gemma-4-",
                "models/gemini-3-",
                "models/gemma-4-",
            ],
        ),
        FeatureId::AnthropicWebSearch => model_starts(
            model_id,
            &[
                "claude-sonnet-4",
                "claude-3-7-sonnet",
                "claude-haiku-4",
                "claude-3-5-haiku",
                "claude-opus-4",
            ],
        ),
        FeatureId::AnthropicInterleavedThinking => {
            let re = Regex::new(r"^claude-(opus|sonnet)-4").expect("static regex");
            re.is_match(&model_id.to_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_compatible_variants() {
        assert!(is_feature_supported(
            FeatureId::OpenAiThinkingBudget,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3-32b"
        ));
        assert!(is_feature_supported(
            FeatureId::OpenAiDeepSeekReasoningEffort,
            "https://example.test/v1",
            "deepseek-v4"
        ));
        assert!(!is_feature_supported(
            FeatureId::OpenAiThinkingStrategy,
            "https://api.openai.com/v1",
            "gpt-4.1"
        ));
    }

    #[test]
    fn infers_native_web_search_capabilities() {
        assert!(native_web_search_supported(
            ProviderProtocol::OpenaiResponses,
            "https://api.openai.com",
            "gpt-5"
        ));
        assert!(native_web_search_supported(
            ProviderProtocol::OpenaiChat,
            "https://api.openai.com",
            "gpt-5-search-api"
        ));
        assert!(!native_web_search_supported(
            ProviderProtocol::OpenaiChat,
            "https://api.openai.com",
            "gpt-5"
        ));
        assert!(native_web_search_supported(
            ProviderProtocol::Gemini,
            "https://generativelanguage.googleapis.com",
            "models/gemini-2.5-pro"
        ));
        assert!(native_web_search_supported(
            ProviderProtocol::VertexAi,
            "https://aiplatform.googleapis.com",
            "publishers/google/models/gemini-2.0-flash"
        ));
        assert!(!native_web_search_supported(
            ProviderProtocol::Ollama,
            "http://localhost:11434/api",
            "qwen3"
        ));
    }

    #[test]
    fn infers_reasoning_and_web_for_known_models() {
        let openai = infer_model_capabilities(
            ProviderProtocol::OpenaiResponses,
            "https://api.openai.com",
            "gpt-5",
        );
        assert!(openai.reasoning);
        assert!(openai.web);

        let qwen = infer_model_capabilities(
            ProviderProtocol::OpenaiChat,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3-235b-a22b",
        );
        assert!(qwen.reasoning);
        assert!(!qwen.web);
    }

    #[test]
    fn derives_supported_thinking_efforts_by_protocol_and_model() {
        assert_eq!(
            supported_thinking_efforts(
                ProviderProtocol::OpenaiChat,
                "https://api.deepseek.com",
                "deepseek-v4",
                true,
            ),
            vec![
                ThinkingEffort::None,
                ThinkingEffort::High,
                ThinkingEffort::Max
            ]
        );
        assert_eq!(
            supported_thinking_efforts(
                ProviderProtocol::Gemini,
                "https://generativelanguage.googleapis.com",
                "gemini-3-pro",
                true,
            ),
            vec![
                ThinkingEffort::None,
                ThinkingEffort::Minimal,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High
            ]
        );
        assert_eq!(
            supported_thinking_efforts(
                ProviderProtocol::Gemini,
                "https://generativelanguage.googleapis.com",
                "gemini-2.5-pro",
                true,
            ),
            vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High
            ]
        );
        assert_eq!(
            supported_thinking_efforts(
                ProviderProtocol::OpenaiResponses,
                "https://api.openai.com",
                "gpt-5",
                true,
            ),
            vec![
                ThinkingEffort::None,
                ThinkingEffort::Minimal,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
                ThinkingEffort::Xhigh
            ]
        );
    }
}

use regex::Regex;
use url::Url;

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

fn baidu_model(base_url: &str, model_id: &str, ids: &[&str]) -> bool {
    provider_is(base_url, &["qianfan.baidubce.com"])
        && ids.iter().any(|id| model_id.eq_ignore_ascii_case(id))
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
        FeatureId::OpenAiDeepSeekReasoningEffort => model_contains(model_id, &["deepseek-v4"]),
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
}

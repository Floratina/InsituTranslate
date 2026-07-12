import type {
  CustomParameterPreset,
  CustomParameterPresetGroup,
} from "./types";

const GLOSSARY_JSON_SCHEMA = {
  type: "array",
  items: {
    type: "object",
    properties: {
      src: { type: "string" },
      dst: { type: "string" },
    },
    required: ["src", "dst"],
    additionalProperties: false,
  },
};

const JSON_MODE_DESCRIPTION = "强制返回合法 JSON，但不保证字段结构";
const GLOSSARY_SCHEMA_DESCRIPTION = "术语表结构已内置，无需在助手提示词中重复填写";

export const ASSISTANT_EMOJIS = [
  "🤖",
  "😀",
  "🧠",
  "✍️",
  "📚",
  "🌐",
  "🔎",
  "🛠️",
  "✨",
  "🧑‍💻",
  "📝",
  "💡",
] as const;

export const ASSISTANT_LUCIDE_ICONS = [
  "bot",
  "languages",
  "book-open",
  "file-check",
  "scan-text",
  "brain",
  "sparkles",
  "wand-sparkles",
  "search",
  "pen-line",
  "braces",
  "messages-square",
] as const;

export const CUSTOM_PARAMETER_PRESET_GROUPS: CustomParameterPresetGroup[] = [
  "通用",
  "OpenAI Chat",
  "OpenAI Responses",
  "DeepSeek",
  "Anthropic",
  "Gemini",
  "Ollama",
];

export const CUSTOM_PARAMETER_PRESETS: CustomParameterPreset[] = [
  {
    group: "通用",
    label: "文本",
    description: "新增一个字符串键值",
    value: { custom_text: "value" },
  },
  {
    group: "通用",
    label: "数字",
    description: "新增一个数值键值",
    value: { custom_number: 1 },
  },
  {
    group: "通用",
    label: "布尔值",
    description: "新增一个开关型键值",
    value: { custom_enabled: true },
  },
  {
    group: "通用",
    label: "JSON 对象",
    description: "新增一个可继续嵌套的对象",
    value: { custom_object: { key: "value" } },
  },
  {
    group: "OpenAI Chat",
    label: "服务层级",
    description: "由服务端自动选择请求处理层级",
    value: { service_tier: "auto" },
  },
  {
    group: "OpenAI Chat",
    label: "存储响应",
    description: "控制是否存储模型响应，预设为关闭",
    value: { store: false },
  },
  {
    group: "OpenAI Chat",
    label: "JSON 模式",
    description: JSON_MODE_DESCRIPTION,
    value: { response_format: { type: "json_object" } },
  },
  {
    group: "OpenAI Chat",
    label: "严格术语表 Schema",
    description: GLOSSARY_SCHEMA_DESCRIPTION,
    value: {
      response_format: {
        type: "json_schema",
        json_schema: {
          name: "glossary",
          strict: true,
          schema: GLOSSARY_JSON_SCHEMA,
        },
      },
    },
    purposes: ["glossary"],
  },
  {
    group: "OpenAI Responses",
    label: "服务层级",
    description: "由服务端自动选择请求处理层级",
    value: { service_tier: "auto" },
  },
  {
    group: "OpenAI Responses",
    label: "存储响应",
    description: "控制是否存储模型响应，预设为关闭",
    value: { store: false },
  },
  {
    group: "OpenAI Responses",
    label: "JSON 模式",
    description: JSON_MODE_DESCRIPTION,
    value: { text: { format: { type: "json_object" } } },
  },
  {
    group: "OpenAI Responses",
    label: "严格术语表 Schema",
    description: GLOSSARY_SCHEMA_DESCRIPTION,
    value: {
      text: {
        format: {
          type: "json_schema",
          name: "glossary",
          strict: true,
          schema: GLOSSARY_JSON_SCHEMA,
        },
      },
    },
    purposes: ["glossary"],
  },
  {
    group: "DeepSeek",
    label: "JSON 模式",
    description: `${JSON_MODE_DESCRIPTION}；可能偶尔返回空内容`,
    value: { response_format: { type: "json_object" } },
  },
  {
    group: "DeepSeek",
    label: "最大输出 Token",
    description: "限制单次响应长度，避免 JSON 中途截断",
    value: { max_tokens: 8192 },
  },
  {
    group: "Anthropic",
    label: "服务层级",
    description: "由服务端自动选择请求处理层级",
    value: { service_tier: "auto" },
  },
  {
    group: "Anthropic",
    label: "Top-K",
    description: "仅从概率最高的候选 Token 中采样",
    value: { top_k: 40 },
  },
  {
    group: "Anthropic",
    label: "请求元数据",
    description: "附加用于识别请求来源的用户标识",
    value: { metadata: { user_id: "insitu-translate" } },
  },
  {
    group: "Anthropic",
    label: "严格术语表 Schema",
    description: `仅受支持的模型可用；${GLOSSARY_SCHEMA_DESCRIPTION}`,
    value: {
      output_config: {
        format: {
          type: "json_schema",
          schema: GLOSSARY_JSON_SCHEMA,
        },
      },
    },
    purposes: ["glossary"],
  },
  {
    group: "Gemini",
    label: "存储请求",
    description: "控制是否存储请求，预设为关闭",
    value: { store: false },
  },
  {
    group: "Gemini",
    label: "生成配置",
    description: "仅生成一个候选回复",
    value: { generationConfig: { candidateCount: 1 } },
  },
  {
    group: "Gemini",
    label: "安全设置",
    description: "设置骚扰内容的过滤阈值",
    value: {
      safetySettings: [
        {
          category: "HARM_CATEGORY_HARASSMENT",
          threshold: "BLOCK_MEDIUM_AND_ABOVE",
        },
      ],
    },
  },
  {
    group: "Gemini",
    label: "JSON 模式",
    description: JSON_MODE_DESCRIPTION,
    value: {
      generationConfig: {
        responseMimeType: "application/json",
      },
    },
  },
  {
    group: "Gemini",
    label: "严格术语表 Schema",
    description: GLOSSARY_SCHEMA_DESCRIPTION,
    value: {
      generationConfig: {
        responseMimeType: "application/json",
        responseJsonSchema: GLOSSARY_JSON_SCHEMA,
      },
    },
    purposes: ["glossary"],
  },
  {
    group: "Ollama",
    label: "JSON 模式",
    description: JSON_MODE_DESCRIPTION,
    value: { format: "json" },
  },
  {
    group: "Ollama",
    label: "模型驻留",
    description: "响应后让模型在内存中保留 5 分钟",
    value: { keep_alive: "5m" },
  },
  {
    group: "Ollama",
    label: "运行参数",
    description: "设置固定随机种子与 Top-K",
    value: { options: { seed: 42, top_k: 40 } },
  },
  {
    group: "Ollama",
    label: "严格术语表 Schema",
    description: GLOSSARY_SCHEMA_DESCRIPTION,
    value: { format: GLOSSARY_JSON_SCHEMA },
    purposes: ["glossary"],
  },
];

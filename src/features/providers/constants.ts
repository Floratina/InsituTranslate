import {
  BookOpen,
  FileCheck2,
  Languages,
  ScanText,
  type LucideIcon,
} from "lucide-react";

import type { ProviderPurpose, ProviderProtocol } from "./types";

export interface PurposeOption {
  value: ProviderPurpose;
  label: string;
  icon: LucideIcon;
}

export const PURPOSES: PurposeOption[] = [
  { value: "translation", label: "翻译", icon: Languages },
  { value: "glossary", label: "术语表", icon: BookOpen },
  { value: "proofreading", label: "校对", icon: FileCheck2 },
  { value: "document-parsing", label: "文档解析", icon: ScanText },
];

export const BUILTIN_AVATARS = new Set([
  "openai",
  "gemini",
  "anthropic",
  "deepseek",
  "qwen",
  "ollama",
  "openrouter",
  "mineru",
  "vertex-ai",
]);

export const AVATAR_LIBRARY: Array<{ name: string; src: string }> = ([
  ["302.AI", "302ai-OYnezl-B.webp"], ["360 智脑", "360-D7q-rf3l.png"],
  ["AIHubMix", "aihubmix-DNVgoSag.webp"], ["AIOnly", "aiOnly-CX5LzR-B.webp"],
  ["AlayaNew", "alayanew-BYgMPG6N.webp"], ["Anthropic", "anthropic-hp89qtrg.png"],
  ["AWS Bedrock", "aws-bedrock-CZfekeIk.webp"], ["Bailian", "bailian-B5l6zvuZ.png"],
  ["BurnCloud", "burncloud-Dv3aLUVa.png"], ["Cephalon", "cephalon-BHAAckMS.jpeg"],
  ["Cerebras", "cerebras-CMYn4Ibf.webp"], ["CherryIN", "cherryin-BIxUyRnC.png"],
  ["DeepSeek", "deepseek-BfIKgrKz.png"], ["DMXAPI", "DMXAPI-DEJq_RKL.png"],
  ["Fireworks", "fireworks-2wG1MQGi.png"], ["Gitee AI", "gitee-ai-C66hc2eY.png"],
  ["GitHub", "github-C6DM68zD.png"], ["Google", "google-C4MGSIHw.png"],
  ["GPUStack", "gpustack-D7EptUU-.svg"], ["Grok", "grok-C9APDUTb.png"],
  ["Groq", "groq-DxjL3oyr.png"], ["Hugging Face", "huggingface-C_i5qDcj.webp"],
  ["Hunyuan", "hunyuan-el6x823I.png"], ["Hyperbolic", "hyperbolic-DbK1JYGQ.png"],
  ["Kimi", "kimi-DRX5773U.webp"], ["LanYun", "lanyun-B8brKWPb.png"],
  ["LM Studio", "lmstudio-BKXYpFdb.png"], ["Mimo", "mimo-pbhfe3Fd.svg"],
  ["MinerU", "MinerU-7Gik6b8.png"],
  ["MiniMax", "minimax-B0Eo-1V9.png"], ["ModelScope", "modelscope-CJyewHiF.png"],
  ["NewAPI", "newapi-9xdaY7q5.png"], ["NVIDIA", "nvidia-oSS9qnh1.png"],
  ["OCoolAI", "ocoolai-BqMN4HQx.png"], ["Ollama", "ollama-BiKnEc5r.png"],
  ["OpenAI", "openai--2_yMGcs.png"], ["OpenRouter", "openrouter-CT0jBAsT.png"],
  ["Perplexity", "perplexity-BwIm93Ua.png"], ["PH8", "ph8-JO6U1vW7.png"],
  ["Qwen", "qwen-2vDMq7H8.png"],
  ["Together AI", "together-0h26j0S1.png"], ["TokenFlux", "tokenflux-CvotEeez.png"],
  ["Vertex AI", "vertex-ai-O6Mq7HyP.png"],
  ["Volcengine", "volcengine-la_PI8m-.png"], ["Voyage AI", "voyageai-UINPkc3N.png"],
  ["Xirang", "xirang-B42-6Dao.png"], ["Zero One", "zero-one-CLSpCDeh.png"],
  ["Zhipu AI", "zhipu-CFgqzqwQ.png"],
] as Array<[string, string]>)
  .map(([name, file]) => ({ name, src: `/provider-library/${file}` }))
  .sort((left, right) => left.name.localeCompare(right.name, "en"));

export const EMPTY_PROVIDER_FORM = {
  name: "",
  protocol: "openai-chat" as ProviderProtocol,
  avatar: null,
};

export const EMPTY_MODEL_FORM = { requestName: "", alias: "" };

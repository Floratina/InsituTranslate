import type { ProviderPurpose } from "@/features/providers/types";

export type AssistantIconKind = "emoji" | "lucide";
export type CustomParameterPresetGroup =
  | "通用"
  | "OpenAI Chat"
  | "OpenAI Responses"
  | "DeepSeek"
  | "Anthropic"
  | "Gemini"
  | "Ollama";

export interface AssistantView {
  id: string;
  name: string;
  iconKind: AssistantIconKind;
  iconValue: string;
  purpose: ProviderPurpose;
  systemPrompt: string;
  temperatureEnabled: boolean;
  temperature: number;
  topPEnabled: boolean;
  topP: number;
  customParameters: Record<string, unknown>;
}

export interface AssistantSettingsDraft {
  id: string;
  name: string;
  iconKind: AssistantIconKind;
  iconValue: string;
  temperatureEnabled: boolean;
  temperature: number;
  topPEnabled: boolean;
  topP: number;
}

export interface CustomParameterPreset {
  group: CustomParameterPresetGroup;
  label: string;
  description: string;
  value: Record<string, unknown>;
  purposes?: ProviderPurpose[];
}

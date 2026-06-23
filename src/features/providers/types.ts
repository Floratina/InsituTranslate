export type ProviderPurpose =
  | "translation"
  | "glossary"
  | "proofreading"
  | "document-parsing";

export type ProviderProtocol =
  | "openai-chat"
  | "openai-responses"
  | "anthropic"
  | "gemini"
  | "vertex-ai"
  | "ollama";

export type MinerUMode = "standard" | "flash";

export interface MinerUProviderConfig {
  mode: MinerUMode;
  flashBaseUrl: string;
}

export interface ProviderConfig {
  mineru?: MinerUProviderConfig;
  vertexAi?: VertexAiProviderConfig;
  [key: string]: unknown;
}

export interface VertexAiProviderConfig {
  projectId: string;
  location: string;
  clientEmail: string;
}

export interface ModelView {
  id: string;
  providerId: string;
  requestName: string;
  alias: string;
  source: string;
  capabilityReasoning: boolean;
  capabilityWeb: boolean;
  capabilityTools: boolean;
  testStatus: string;
  latencyMs: number | null;
  testedAt: string | null;
  testError: string | null;
}

export interface ProviderView {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string;
  useRawBaseUrl: boolean;
  config: ProviderConfig;
  avatar: string | null;
  isBuiltin: boolean;
  enabled: boolean;
  credentialMask: string | null;
  customHeaderKeys: string[];
  purpose: ProviderPurpose;
  models: ModelView[];
}

export interface RemoteModel {
  requestName: string;
  alias: string;
  added: boolean;
}

export interface ProviderDraft {
  id: string;
  baseUrl: string;
  useRawBaseUrl: boolean;
  config: ProviderConfig;
}

export interface ProviderForm {
  name: string;
  protocol: ProviderProtocol;
  avatar: string | null;
}

export interface NewModelForm {
  requestName: string;
  alias: string;
}

export interface ConnectivityResult {
  success: boolean;
  latencyMs: number;
  testedAt: string;
  error: string | null;
}

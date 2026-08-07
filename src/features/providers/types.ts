export type ProviderPurpose =
  | "translation"
  | "glossary"
  | "proofreading"
  | "document-parsing";

export type ProviderProtocol = string;

export type ProtocolStatus = "available" | "unknown";

export interface ProtocolDescriptor {
  id: ProviderProtocol;
  displayName: string;
  defaultBaseUrl: string;
  wireFamily: string;
  configKind: "generic" | "vertex-ai" | string;
  supportsModelListing: boolean;
  auth: ProtocolAuthDescriptor;
  configFields: ProtocolConfigField[];
  helpText: string | null;
}

export interface ProtocolAuthDescriptor {
  kind: string;
  label: string;
  header: string;
  helpText: string | null;
}

export type ProtocolConfigFieldKind = "text" | "number" | "boolean" | "select";

export interface ProtocolConfigOption {
  value: string;
  label: string;
}

export interface ProtocolConfigField {
  pointer: string;
  label: string;
  kind: ProtocolConfigFieldKind;
  required: boolean;
  defaultValue: unknown;
  options: ProtocolConfigOption[];
  helpText: string | null;
}

export interface ProtocolEndpointPreview {
  chat: string;
  models: string | null;
}

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

export interface ProviderConfigIssue {
  pointer: string;
  message: string;
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
  capabilities: Record<string, CapabilityValue>;
  testStatus: string;
  latencyMs: number | null;
  testedAt: string | null;
  testError: string | null;
}

export type ThinkingEffort =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export type CapabilityValue = boolean | ThinkingEffort[] | ThinkingEffort | null;

export type CapabilityValueKind =
  | "boolean"
  | "thinking-efforts"
  | "optional-thinking-effort";

export type CapabilityEditor = "toggle" | "select" | "hidden";
export type CapabilityPresentation = "badge" | "hidden";

export interface CapabilityOptionDescriptor {
  value: string;
  label: string;
}

export interface CapabilityDescriptor {
  id: string;
  label: string;
  description: string;
  valueKind: CapabilityValueKind;
  userEditable: boolean;
  icon: string | null;
  editor: CapabilityEditor;
  presentation: CapabilityPresentation;
  options: CapabilityOptionDescriptor[];
  defaultValue: CapabilityValue;
}

export interface ProviderView {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  protocolStatus: ProtocolStatus;
  protocolRawId: string | null;
  baseUrl: string;
  useRawBaseUrl: boolean;
  config: ProviderConfig;
  configIssues: ProviderConfigIssue[];
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

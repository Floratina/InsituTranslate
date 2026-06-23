import type { ProviderConfig, VertexAiProviderConfig } from "./types";

export const VERTEX_AI_DEFAULT_BASE_URL = "https://aiplatform.googleapis.com";
export const VERTEX_AI_DEFAULT_LOCATION = "global";

export interface VertexAiLocationOption {
  value: string;
  label: string;
}

export interface UpdateVertexAiConfigInput {
  providerId: string;
  projectId: string;
  location: string;
  clientEmail: string;
  privateKey?: string | null;
}

export interface ImportVertexAiServiceAccountInput {
  providerId: string;
  serviceAccountJson: string;
  location?: string | null;
}

export const VERTEX_AI_LOCATIONS: VertexAiLocationOption[] = [
  "global",
  "us-central1",
  "us-east1",
  "us-east4",
  "us-east5",
  "us-south1",
  "us-west1",
  "us-west2",
  "us-west3",
  "us-west4",
  "northamerica-northeast1",
  "northamerica-northeast2",
  "southamerica-east1",
  "southamerica-west1",
  "europe-central2",
  "europe-north1",
  "europe-southwest1",
  "europe-west1",
  "europe-west2",
  "europe-west3",
  "europe-west4",
  "europe-west6",
  "europe-west8",
  "europe-west9",
  "europe-west10",
  "europe-west12",
  "asia-east1",
  "asia-east2",
  "asia-northeast1",
  "asia-northeast2",
  "asia-northeast3",
  "asia-south1",
  "asia-south2",
  "asia-southeast1",
  "asia-southeast2",
  "australia-southeast1",
  "australia-southeast2",
  "me-central1",
  "me-central2",
  "me-west1",
  "africa-south1",
].map((location) => ({ value: location, label: location }));

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stringField(value: unknown): string {
  return typeof value === "string" ? value : "";
}

export function getVertexAiConfig(config: ProviderConfig): VertexAiProviderConfig {
  const vertex: Record<string, unknown> = isRecord(config.vertexAi) ? config.vertexAi : {};
  return {
    projectId: stringField(vertex.projectId).trim(),
    location: stringField(vertex.location).trim() || VERTEX_AI_DEFAULT_LOCATION,
    clientEmail: stringField(vertex.clientEmail).trim(),
  };
}

export function vertexAiPreviewBaseUrl(
  baseUrl: string,
  config: Pick<VertexAiProviderConfig, "projectId" | "location">,
): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, "");
  if (!trimmed) return "请先填写 Base URL";
  const project = config.projectId.trim() || "my-project";
  const location = config.location.trim() || VERTEX_AI_DEFAULT_LOCATION;
  const isDefaultAiplatform =
    /^https?:\/\/aiplatform\.googleapis\.com(?:\/(?:v1|v1beta1)?)?\/?$/i.test(trimmed);
  const serviceBase = isDefaultAiplatform
    ? location === VERTEX_AI_DEFAULT_LOCATION
      ? VERTEX_AI_DEFAULT_BASE_URL
      : `https://${location}-aiplatform.googleapis.com`
    : trimmed;
  const normalized = serviceBase.replace(/\/+$/, "");
  if (normalized.includes("/projects/") && normalized.includes("/locations/")) {
    return `预览: ${normalized.replace(/\/publishers\/google(?:\/models)?$/, "")}/publishers/google/models/{model}:generateContent`;
  }
  const root = normalized.replace(/\/v1(?:beta1)?$/, "");
  return `预览: ${root}/v1/projects/${project}/locations/${location}/publishers/google/models/{model}:generateContent`;
}

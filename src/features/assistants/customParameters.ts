export const JSON_OUTPUT_CONFLICT_WARNING =
  "对部分提供商（如 OpenAI、DeepSeek 和 Ollama），Schema和JSON对象不应同时插入到请求体，请检查您的填写";

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function deepMerge(
  current: Record<string, unknown>,
  incoming: Record<string, unknown>,
): Record<string, unknown> {
  const merged: Record<string, unknown> = structuredClone(current);
  for (const [key, value] of Object.entries(incoming)) {
    merged[key] =
      isRecord(merged[key]) && isRecord(value)
        ? deepMerge(merged[key], value)
        : structuredClone(value);
  }
  return merged;
}

export function hasJsonOutputConflict(parameters: Record<string, unknown>): boolean {
  let hasJsonMode = false;
  let hasSchema = false;

  const chatFormat = parameters.response_format;
  if (isRecord(chatFormat)) {
    hasJsonMode ||= chatFormat.type === "json_object";
    hasSchema ||= chatFormat.type === "json_schema" || isRecord(chatFormat.json_schema);
  }

  const responsesText = parameters.text;
  if (isRecord(responsesText) && isRecord(responsesText.format)) {
    hasJsonMode ||= responsesText.format.type === "json_object";
    hasSchema ||=
      responsesText.format.type === "json_schema" ||
      isRecord(responsesText.format.schema);
  }

  const anthropicOutput = parameters.output_config;
  if (isRecord(anthropicOutput) && isRecord(anthropicOutput.format)) {
    hasSchema ||=
      anthropicOutput.format.type === "json_schema" ||
      isRecord(anthropicOutput.format.schema);
  }

  const generationConfig = parameters.generationConfig;
  if (isRecord(generationConfig)) {
    const geminiHasSchema =
      isRecord(generationConfig.responseJsonSchema) ||
      isRecord(generationConfig.responseSchema);
    hasSchema ||= geminiHasSchema;
    hasJsonMode ||=
      !geminiHasSchema && generationConfig.responseMimeType === "application/json";
  }

  if (parameters.format === "json") {
    hasJsonMode = true;
  } else if (isRecord(parameters.format)) {
    hasSchema = true;
  }

  return hasJsonMode && hasSchema;
}

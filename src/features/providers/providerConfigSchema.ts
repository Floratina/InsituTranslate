import type {
  ProtocolConfigField,
  ProviderConfig,
  ProviderConfigIssue,
} from "./types";

export function pointerSegments(pointer: string): string[] {
  return pointer
    .slice(1)
    .split("/")
    .map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
}

export function pointerValue(config: ProviderConfig, pointer: string): unknown {
  let value: unknown = config;
  for (const segment of pointerSegments(pointer)) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
    value = (value as Record<string, unknown>)[segment];
  }
  return value;
}

export function withPointerValue(
  config: ProviderConfig,
  pointer: string,
  nextValue: unknown,
): ProviderConfig {
  const segments = pointerSegments(pointer);
  const root: Record<string, unknown> = { ...config };
  let current = root;
  for (const segment of segments.slice(0, -1)) {
    const child = current[segment];
    const next = typeof child === "object" && child !== null && !Array.isArray(child)
      ? { ...(child as Record<string, unknown>) }
      : {};
    current[segment] = next;
    current = next;
  }
  const leaf = segments.at(-1);
  if (leaf !== undefined) current[leaf] = nextValue;
  return root;
}

export function withoutPointerValue(
  config: ProviderConfig,
  pointer: string,
): ProviderConfig {
  const segments = pointerSegments(pointer);
  const root: Record<string, unknown> = { ...config };
  let current = root;
  for (const segment of segments.slice(0, -1)) {
    const child = current[segment];
    if (typeof child !== "object" || child === null || Array.isArray(child)) return root;
    const next = { ...(child as Record<string, unknown>) };
    current[segment] = next;
    current = next;
  }
  const leaf = segments.at(-1);
  if (leaf !== undefined) delete current[leaf];
  return root;
}

export function validateProviderConfig(
  fields: ProtocolConfigField[],
  config: ProviderConfig,
): ProviderConfigIssue[] {
  const issues: ProviderConfigIssue[] = [];
  for (const field of fields) {
    const value = pointerValue(config, field.pointer);
    if (value === undefined) {
      if (field.required) {
        issues.push({ pointer: field.pointer, message: `${field.label}为必填项` });
      }
      continue;
    }
    if (value === null) {
      issues.push({
        pointer: field.pointer,
        message: field.required ? `${field.label}为必填项` : `${field.label}不能为 null`,
      });
      continue;
    }
    if (field.kind === "text") {
      if (typeof value !== "string") {
        issues.push({ pointer: field.pointer, message: `${field.label}必须是文本` });
      } else if (field.required && !value.trim()) {
        issues.push({ pointer: field.pointer, message: `${field.label}为必填项` });
      }
      continue;
    }
    if (field.kind === "number") {
      if (typeof value !== "number" || !Number.isFinite(value)) {
        issues.push({ pointer: field.pointer, message: `${field.label}必须是数字` });
      }
      continue;
    }
    if (field.kind === "boolean") {
      if (typeof value !== "boolean") {
        issues.push({ pointer: field.pointer, message: `${field.label}必须是布尔值` });
      }
      continue;
    }
    if (
      typeof value !== "string"
      || !field.options.some((option) => option.value === value)
    ) {
      issues.push({ pointer: field.pointer, message: `${field.label}必须使用可用选项` });
    }
  }
  return issues;
}

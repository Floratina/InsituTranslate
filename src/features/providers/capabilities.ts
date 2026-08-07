import { iconNames, type IconName } from "lucide-react/dynamic";

import type {
  CapabilityDescriptor,
  CapabilityValue,
  ModelView,
  ThinkingEffort,
} from "./types";

export const CAPABILITY_IDS = {
  reasoning: "reasoning",
  web: "web",
  thinkingEffort: "thinking-effort",
  thinkingRequired: "thinking-required",
  defaultThinkingEffort: "default-thinking-effort",
} as const;

const THINKING_EFFORTS = new Set<ThinkingEffort>([
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
]);
const ICON_NAMES = new Set<string>(iconNames);

function capabilityValue(
  capabilities: Record<string, CapabilityValue>,
  id: string,
): CapabilityValue {
  if (!(id in capabilities)) {
    throw new Error(`Capability map is missing registered capability: ${id}`);
  }
  return capabilities[id];
}

export function booleanCapability(
  capabilities: Record<string, CapabilityValue>,
  id: string,
): boolean {
  const value = capabilityValue(capabilities, id);
  if (typeof value !== "boolean") {
    throw new Error(`Capability ${id} must be a boolean`);
  }
  return value;
}

export function reasoningCapability(model: ModelView): boolean {
  return booleanCapability(model.capabilities, CAPABILITY_IDS.reasoning);
}

export function webCapability(model: ModelView): boolean {
  return booleanCapability(model.capabilities, CAPABILITY_IDS.web);
}

export function thinkingRequiredCapability(model: ModelView): boolean {
  return booleanCapability(model.capabilities, CAPABILITY_IDS.thinkingRequired);
}

export function thinkingEffortsCapability(model: ModelView): ThinkingEffort[] {
  const value = capabilityValue(model.capabilities, CAPABILITY_IDS.thinkingEffort);
  if (!Array.isArray(value) || !value.every((effort) => THINKING_EFFORTS.has(effort))) {
    throw new Error("Capability thinking-effort must be a valid thinking effort list");
  }
  return value;
}

export function defaultThinkingEffortCapability(model: ModelView): ThinkingEffort | null {
  const value = capabilityValue(model.capabilities, CAPABILITY_IDS.defaultThinkingEffort);
  if (value !== null && (typeof value !== "string" || !THINKING_EFFORTS.has(value as ThinkingEffort))) {
    throw new Error("Capability default-thinking-effort must be a valid optional thinking effort");
  }
  return value as ThinkingEffort | null;
}

export function capabilityIconName(descriptor: CapabilityDescriptor): IconName {
  if (!descriptor.icon || !ICON_NAMES.has(descriptor.icon)) {
    throw new Error(`Capability descriptor ${descriptor.id} has an invalid Lucide icon: ${descriptor.icon ?? "null"}`);
  }
  return descriptor.icon as IconName;
}

export function validateCapabilityDescriptors(
  descriptors: CapabilityDescriptor[],
): CapabilityDescriptor[] {
  const ids = new Set<string>();
  for (const descriptor of descriptors) {
    if (!descriptor.id || ids.has(descriptor.id)) {
      throw new Error(`Capability descriptor ID is empty or duplicated: ${descriptor.id}`);
    }
    ids.add(descriptor.id);
    if (descriptor.presentation === "badge" || descriptor.editor !== "hidden") {
      capabilityIconName(descriptor);
    }
    if (descriptor.presentation === "badge" && descriptor.valueKind !== "boolean") {
      throw new Error(`Badge capability ${descriptor.id} must have a boolean value`);
    }
    if (descriptor.editor !== "hidden" && !descriptor.userEditable) {
      throw new Error(`Capability ${descriptor.id} has an editor but is not user-editable`);
    }
  }
  for (const [id, valueKind] of [
    [CAPABILITY_IDS.reasoning, "boolean"],
    [CAPABILITY_IDS.web, "boolean"],
    [CAPABILITY_IDS.thinkingEffort, "thinking-efforts"],
    [CAPABILITY_IDS.thinkingRequired, "boolean"],
    [CAPABILITY_IDS.defaultThinkingEffort, "optional-thinking-effort"],
  ] as const) {
    const descriptor = descriptors.find((item) => item.id === id);
    if (!descriptor || descriptor.valueKind !== valueKind) {
      throw new Error(`Capability descriptor ${id} is missing or has the wrong value type`);
    }
  }
  return descriptors;
}

export function editableCapabilityValues(
  capabilities: Record<string, CapabilityValue>,
  descriptors: CapabilityDescriptor[],
): Record<string, CapabilityValue> {
  return Object.fromEntries(
    descriptors
      .filter((descriptor) => descriptor.userEditable)
      .map((descriptor) => [descriptor.id, capabilityValue(capabilities, descriptor.id)]),
  );
}

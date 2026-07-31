import { HelpTooltip } from "@/components/ui/help-tooltip";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

import type { ProviderConfig, ProtocolConfigField } from "./types";

interface ProtocolConfigFieldsProps {
  fields: ProtocolConfigField[];
  config: ProviderConfig;
  onChange: (config: ProviderConfig) => void;
}

function pointerSegments(pointer: string): string[] {
  return pointer
    .slice(1)
    .split("/")
    .map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
}

function pointerValue(config: ProviderConfig, pointer: string): unknown {
  let value: unknown = config;
  for (const segment of pointerSegments(pointer)) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
    value = (value as Record<string, unknown>)[segment];
  }
  return value;
}

function withPointerValue(
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
  if (leaf) current[leaf] = nextValue;
  return root;
}

function FieldLabel({ field }: { field: ProtocolConfigField }) {
  return (
    <div className="flex items-center gap-1.5">
      <Label className="text-sm">
        {field.label}
        {field.required ? " *" : ""}
      </Label>
      {field.helpText && <HelpTooltip contentClassName="max-w-80">{field.helpText}</HelpTooltip>}
    </div>
  );
}

export function ProtocolConfigFields({ fields, config, onChange }: ProtocolConfigFieldsProps) {
  if (fields.length === 0) return null;

  return (
    <div className="grid gap-2">
      {fields.map((field) => {
        const value = pointerValue(config, field.pointer) ?? field.defaultValue;
        if (field.kind === "boolean") {
          return (
            <div key={field.pointer} className="flex items-center justify-between gap-3">
              <FieldLabel field={field} />
              <Switch
                checked={value === true}
                onCheckedChange={(checked) =>
                  onChange(withPointerValue(config, field.pointer, checked))
                }
              />
            </div>
          );
        }
        if (field.kind === "select") {
          return (
            <div key={field.pointer} className="grid gap-1">
              <FieldLabel field={field} />
              <Select
                value={typeof value === "string" ? value : ""}
                onValueChange={(selected) =>
                  onChange(withPointerValue(config, field.pointer, selected))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {field.options.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          );
        }
        return (
          <div key={field.pointer} className="grid gap-1">
            <FieldLabel field={field} />
            <Input
              type={field.kind === "number" ? "number" : "text"}
              value={typeof value === "string" || typeof value === "number" ? value : ""}
              onChange={(event) => {
                const nextValue = field.kind === "number"
                  ? Number(event.target.value)
                  : event.target.value;
                onChange(withPointerValue(config, field.pointer, nextValue));
              }}
            />
          </div>
        );
      })}
    </div>
  );
}

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

import {
  pointerValue,
  validateProviderConfig,
  withoutPointerValue,
  withPointerValue,
} from "./providerConfigSchema";
import type { ProviderConfig, ProtocolConfigField } from "./types";

interface ProtocolConfigFieldsProps {
  fields: ProtocolConfigField[];
  config: ProviderConfig;
  onChange: (config: ProviderConfig) => void;
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
  const issues = validateProviderConfig(fields, config);

  return (
    <div className="grid gap-2">
      {fields.map((field) => {
        const value = pointerValue(config, field.pointer);
        const issue = issues.find((item) => item.pointer === field.pointer);
        if (field.kind === "boolean") {
          return (
            <div key={field.pointer} className="grid gap-1">
              <div className="flex items-center justify-between gap-3">
                <FieldLabel field={field} />
                <Switch
                  checked={value === true}
                  onCheckedChange={(checked) =>
                    onChange(withPointerValue(config, field.pointer, checked))
                  }
                />
              </div>
              {issue && <div className="text-xs text-destructive">{issue.message}</div>}
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
              {issue && <div className="text-xs text-destructive">{issue.message}</div>}
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
                if (field.kind === "number" && event.target.value === "") {
                  onChange(withoutPointerValue(config, field.pointer));
                  return;
                }
                const nextValue = field.kind === "number"
                  ? Number(event.target.value)
                  : event.target.value;
                onChange(withPointerValue(config, field.pointer, nextValue));
              }}
            />
            {issue && <div className="text-xs text-destructive">{issue.message}</div>}
          </div>
        );
      })}
    </div>
  );
}

import { Bot } from "lucide-react";

import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { AssistantIcon } from "@/features/assistants/AssistantIcon";
import type { AssistantView } from "@/features/assistants/types";
import { ProviderAvatar } from "@/features/providers/ProviderAvatar";
import type { ModelView, ProviderView } from "@/features/providers/types";

interface ModelAssistantSettingsCardProps {
  providers: ProviderView[];
  models: ModelView[];
  assistants: AssistantView[];
  providerId: string;
  modelId: string;
  assistantId: string;
  loading: boolean;
  onProviderChange: (value: string) => void;
  onModelChange: (value: string) => void;
  onAssistantChange: (value: string) => void;
}

export function ModelAssistantSettingsCard({
  providers,
  models,
  assistants,
  providerId,
  modelId,
  assistantId,
  loading,
  onProviderChange,
  onModelChange,
  onAssistantChange,
}: ModelAssistantSettingsCardProps) {
  return (
    <Card size="sm" className="flex h-full flex-col rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex items-center gap-2">
          <Bot className="size-4 text-primary" />
          <CardTitle>模型和助手</CardTitle>
        </div>
      </CardHeader>
      <CardContent className="grid flex-1 content-start gap-3 px-3">
        <div className="grid gap-2">
          <Label>提供商</Label>
          <Select value={providerId} onValueChange={onProviderChange} disabled={loading}>
            <SelectTrigger>
              <SelectValue placeholder="选择提供商" />
            </SelectTrigger>
            <SelectContent>
              {providers.map((provider) => (
                <SelectItem key={provider.id} value={provider.id}>
                  <span className="flex items-center gap-2">
                    <ProviderAvatar
                      name={provider.name}
                      avatar={provider.avatar}
                      className="size-4 text-[7px]"
                    />
                    <span>{provider.name}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-2">
          <Label>模型</Label>
          <Select value={modelId} onValueChange={onModelChange} disabled={models.length === 0}>
            <SelectTrigger>
              <SelectValue placeholder="选择模型" />
            </SelectTrigger>
            <SelectContent>
              {models.map((model) => (
                <SelectItem key={model.id} value={model.id}>
                  {model.alias || model.requestName}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-2">
          <Label>助手</Label>
          <Select value={assistantId} onValueChange={onAssistantChange}>
            <SelectTrigger>
              <SelectValue placeholder="选择助手" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__none__">
                <span className="flex items-center gap-2">
                  <Bot className="size-4 text-muted-foreground" />
                  <span>不使用助手</span>
                </span>
              </SelectItem>
              {assistants.map((assistant) => (
                <SelectItem key={assistant.id} value={assistant.id}>
                  <span className="flex items-center gap-2">
                    <AssistantIcon
                      kind={assistant.iconKind}
                      value={assistant.iconValue}
                      className="size-4 border-0 bg-transparent text-xs"
                      glyphClassName="size-3.5"
                    />
                    <span>{assistant.name}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </CardContent>
    </Card>
  );
}

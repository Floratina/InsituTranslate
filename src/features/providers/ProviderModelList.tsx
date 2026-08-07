import {
  Activity,
  CloudDownload,
  LoaderCircle,
  Plus,
  Settings,
} from "lucide-react";
import { DynamicIcon } from "lucide-react/dynamic";

import { Button } from "@/components/ui/button";
import { HelpTooltip } from "@/components/ui/help-tooltip";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";

import { booleanCapability, capabilityIconName } from "./capabilities";
import type { CapabilityDescriptor, ModelView } from "./types";

interface ProviderModelListProps {
  models: ModelView[];
  capabilityDescriptors: CapabilityDescriptor[];
  testingModelId: string;
  disabled?: boolean;
  supportsModelListing?: boolean;
  onOpenRemoteModels: () => void;
  onAddModel: () => void;
  onTestModel: (model: ModelView) => void;
  onOpenSettings: (model: ModelView) => void;
  variant?: "default" | "mineru";
}

interface CapabilityPillProps {
  descriptor: CapabilityDescriptor;
  label: string;
  active: boolean;
}

function latencyClassName(latencyMs: number | null): string {
  if (latencyMs === null) return "text-muted-foreground";
  if (latencyMs <= 2000) return "text-latency-good";
  if (latencyMs <= 5000) return "text-latency-warning";
  return "text-latency-danger";
}

function CapabilityPill({ descriptor, label, active }: CapabilityPillProps) {
  return (
    <span
      className={cn(
        "inline-flex h-6 items-center gap-1 rounded-[6px] border px-2 text-2xs text-muted-foreground",
        active && "border-enabled-accent/30 bg-enabled-accent/15 text-enabled-accent",
      )}
    >
      <DynamicIcon name={capabilityIconName(descriptor)} className="size-3" strokeWidth={1.8} />
      {label}
    </span>
  );
}

export function ProviderModelList({
  models,
  capabilityDescriptors,
  testingModelId,
  disabled = false,
  supportsModelListing = true,
  onOpenRemoteModels,
  onAddModel,
  onTestModel,
  onOpenSettings,
  variant = "default",
}: ProviderModelListProps) {
  const isMinerU = variant === "mineru";

  return (
    <section className="min-h-64 overflow-hidden rounded-[6px] border">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b p-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <div className="text-sm font-semibold">模型列表</div>
          {isMinerU && (
            <HelpTooltip>
              默认使用 vlm。也可以手动添加 pipeline、MinerU-HTML，或未来官方新增的 model_version。
            </HelpTooltip>
          )}
        </div>
        <div className="flex shrink-0 gap-1">
          <Button
            disabled={disabled || !supportsModelListing}
            variant="outline"
            size="sm"
            onClick={onOpenRemoteModels}
          >
            <CloudDownload className="size-3.5" />
            获取模型列表
          </Button>
          <Button disabled={disabled} variant="outline" size="icon-sm" onClick={onAddModel}>
            <Plus className="size-4" />
          </Button>
        </div>
      </div>
      <Table className="table-fixed">
        <TableHeader>
          <TableRow>
            <TableHead className="h-8 w-[38%] text-xs">模型名称</TableHead>
            <TableHead className="h-8 w-[38%] text-xs">
              {isMinerU ? "解析参数" : "模型能力"}
            </TableHead>
            <TableHead className="h-8 text-right text-xs">测试与设置</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {models.length === 0 ? (
            <TableRow>
              <TableCell colSpan={3} className="h-28 text-center text-xs text-muted-foreground">
                暂无模型，请获取上游模型列表或手动添加
              </TableCell>
            </TableRow>
          ) : (
            models.map((model) => (
              <TableRow key={model.id}>
                <TableCell className="min-w-0 py-2">
                  <div className="truncate text-sm font-medium">{model.alias}</div>
                  <div className="truncate text-2xs text-muted-foreground">
                    {model.requestName}
                  </div>
                </TableCell>
                <TableCell className="py-2">
                  {isMinerU ? (
                    <span className="inline-flex h-6 items-center rounded-[6px] border border-enabled-accent/30 bg-enabled-accent/15 px-2 text-2xs text-enabled-accent">
                      model_version
                    </span>
                  ) : (
                    <div className="flex flex-wrap gap-1">
                      {capabilityDescriptors
                        .filter((descriptor) => descriptor.presentation === "badge")
                        .map((descriptor) => (
                          <CapabilityPill
                            key={descriptor.id}
                            descriptor={descriptor}
                            label={descriptor.label}
                            active={booleanCapability(model.capabilities, descriptor.id)}
                          />
                        ))}
                    </div>
                  )}
                </TableCell>
                <TableCell className="py-2">
                  <div className="flex items-center justify-end gap-1">
                    {model.testStatus === "success" && (
                      <span className={cn("mr-2 text-3xs", latencyClassName(model.latencyMs))}>
                        {model.latencyMs === null ? "-" : `${model.latencyMs}ms`}
                      </span>
                    )}
                    {model.testStatus === "failed" && (
                      <span className="mr-2 text-3xs text-destructive">失败</span>
                    )}
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      disabled={disabled || testingModelId === model.id}
                      title={isMinerU ? "测试 MinerU 连通性" : "测试连通性"}
                      onClick={() => onTestModel(model)}
                    >
                      {testingModelId === model.id ? (
                        <LoaderCircle className="size-4 animate-spin" />
                      ) : (
                        <Activity className="size-4" />
                      )}
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      disabled={!isMinerU && capabilityDescriptors.length === 0}
                      title="模型设置"
                      onClick={() => onOpenSettings(model)}
                    >
                      <Settings className="size-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </section>
  );
}

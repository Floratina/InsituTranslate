import {
  Activity,
  Brain,
  CloudDownload,
  Globe2,
  LoaderCircle,
  Plus,
  Settings,
  Wrench,
  type LucideIcon,
} from "lucide-react";

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

import type { ModelView } from "./types";

interface ProviderModelListProps {
  models: ModelView[];
  testingModelId: string;
  onOpenRemoteModels: () => void;
  onAddModel: () => void;
  onTestModel: (model: ModelView) => void;
  onOpenSettings: (model: ModelView) => void;
  variant?: "default" | "mineru";
}

interface CapabilityPillProps {
  icon: LucideIcon;
  label: string;
  active: boolean;
}

function latencyClassName(latencyMs: number | null): string {
  if (latencyMs === null) return "text-muted-foreground";
  if (latencyMs <= 2000) return "text-latency-good";
  if (latencyMs <= 5000) return "text-latency-warning";
  return "text-latency-danger";
}

function CapabilityPill({ icon: Icon, label, active }: CapabilityPillProps) {
  return (
    <span
      className={cn(
        "inline-flex h-6 items-center gap-1 rounded-[6px] border px-2 text-2xs text-muted-foreground",
        active && "border-enabled-accent/30 bg-enabled-accent/15 text-enabled-accent",
      )}
    >
      <Icon className="size-3" strokeWidth={1.8} />
      {label}
    </span>
  );
}

export function ProviderModelList({
  models,
  testingModelId,
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
          <Button variant="outline" size="sm" onClick={onOpenRemoteModels}>
            <CloudDownload className="size-3.5" />
            获取模型列表
          </Button>
          <Button variant="outline" size="icon-sm" onClick={onAddModel}>
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
                      <CapabilityPill icon={Brain} label="推理" active={model.capabilityReasoning} />
                      <CapabilityPill icon={Globe2} label="联网" active={model.capabilityWeb} />
                      <CapabilityPill icon={Wrench} label="工具调用" active={model.capabilityTools} />
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
                      disabled={testingModelId === model.id}
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

import type { TranslationTaskStatus } from "./types";

export function formatTokenK(value: number): string {
  return `${(value / 1000).toFixed(1)}k`;
}

export function formatPercent(value: number): string {
  return `${Math.round(Math.max(0, Math.min(1, value)) * 100)}%`;
}

export function formatErrorRate(value: number): string {
  return `${(Math.max(0, value) * 100).toFixed(1)}%`;
}

export function statusLabel(status: TranslationTaskStatus): string {
  const labels: Record<TranslationTaskStatus, string> = {
    pending: "待开始",
    running: "进行中",
    interrupted: "中断",
    failed: "失败",
    success: "完成",
  };
  return labels[status];
}

export function unixTimeLabel(value: string): string {
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds <= 0) return "-";
  return new Date(seconds * 1000).toLocaleString();
}

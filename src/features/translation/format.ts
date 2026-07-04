import type { ProgressStep, TranslationTaskStatus, TranslationTaskView } from "./types";

export type TaskStatusSeverity = "muted" | "warning" | "danger";

export interface TaskStatusMessage {
  text: string;
  severity: TaskStatusSeverity;
}

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
    "interrupted-pending": "正在中断",
    interrupted: "中断",
    failed: "失败",
    success: "完成",
  };
  return labels[status];
}

function hasRateLimitSignal(value: string): boolean {
  return /RESOURCE_EXHAUSTED|rate_limited=true|Rate limit reached|quota|配额|频率/i.test(value);
}

const httpErrorMessages: Partial<Record<number, TaskStatusMessage>> = {
  400: {
    text: "400：请求参数不被服务接受，请检查模型或自定义参数配置",
    severity: "danger",
  },
  401: {
    text: "401：认证失败，请检查 API Key 或登录凭证",
    severity: "danger",
  },
  403: {
    text: "403：权限不足或当前账号无模型访问权限",
    severity: "danger",
  },
  404: {
    text: "404：接口或模型不存在，请检查 Base URL 和模型名称",
    severity: "danger",
  },
  408: {
    text: "408：请求超时，可稍后重试或降低并发",
    severity: "warning",
  },
  413: {
    text: "413：请求内容过大，请降低分块大小后重试",
    severity: "danger",
  },
  422: {
    text: "422：请求内容无法被服务处理，请检查模型参数",
    severity: "danger",
  },
  429: {
    text: "429：请求频率或配额达到限制，可稍后继续",
    severity: "warning",
  },
  500: {
    text: "500：服务商接口暂时异常，可稍后重试",
    severity: "warning",
  },
  502: {
    text: "502：服务商接口暂时异常，可稍后重试",
    severity: "warning",
  },
  503: {
    text: "503：服务商接口暂时异常，可稍后重试",
    severity: "warning",
  },
  504: {
    text: "504：服务商接口暂时异常，可稍后重试",
    severity: "warning",
  },
};

function httpStatusMessage(value: string): TaskStatusMessage | null {
  const match = /\bHTTP\s*(\d{3})\b/i.exec(value);
  if (!match) return null;
  const status = Number(match[1]);
  return httpErrorMessages[status] ?? null;
}

function failedProgressStep(task: TranslationTaskView): ProgressStep | null {
  if (!task.progressDetail) return null;
  const steps = [
    task.progressDetail.ast,
    task.progressDetail.chunking,
    task.progressDetail.glossary,
    task.progressDetail.translating,
  ];
  return steps.find((step) => step.state === "failed") ?? null;
}

function localizeTaskError(value: string): TaskStatusMessage {
  const httpMessage = httpStatusMessage(value);
  if (httpMessage) return httpMessage;

  if (hasRateLimitSignal(value)) {
    return {
      text: "请求频率或配额达到限制，可稍后继续",
      severity: "warning",
    };
  }
  if (/placeholder|占位符|restore/i.test(value)) {
    return {
      text: "占位符恢复失败，请检查任务详情",
      severity: "danger",
    };
  }
  if (/error rate|错误率/i.test(value)) {
    return {
      text: "错误率过高，任务已停止",
      severity: "danger",
    };
  }
  if (/parse|AST|chunk|解析|分块/i.test(value)) {
    return {
      text: "文档解析或分块失败，请检查源文件",
      severity: "danger",
    };
  }
  if (/Task paused|interrupted|cancel/i.test(value)) {
    return {
      text: "任务已中断，可继续",
      severity: "warning",
    };
  }
  return {
    text: `任务异常：${value}`,
    severity: "danger",
  };
}

export function taskStatusMessage(task: TranslationTaskView): TaskStatusMessage {
  const lastError = task.lastError?.trim();
  if (lastError) return localizeTaskError(lastError);

  const rateLimitStatus = task.rateLimitStatus?.trim();
  if (rateLimitStatus) {
    const httpMessage = httpStatusMessage(rateLimitStatus);
    if (httpMessage) return httpMessage;
    if (hasRateLimitSignal(rateLimitStatus)) return localizeTaskError(rateLimitStatus);
  }

  const failedStep = failedProgressStep(task);
  if (failedStep) {
    return {
      text: failedStep.label,
      severity: "danger",
    };
  }

  if (task.status === "running" && task.progressDetail?.translating) {
    return {
      text: task.progressDetail.translating.label,
      severity: "muted",
    };
  }
  if (task.status === "interrupted-pending") {
    return {
      text: "正在中断，等待当前请求结束",
      severity: "warning",
    };
  }
  if (task.status === "interrupted") {
    return {
      text: "任务已中断，可继续",
      severity: "warning",
    };
  }
  if (task.status === "failed") {
    return {
      text: "任务失败，请检查任务详情",
      severity: "danger",
    };
  }
  if (task.status === "success") {
    return {
      text: `翻译完成 (${task.completedChunks}/${task.totalChunks})`,
      severity: "muted",
    };
  }
  return {
    text: statusLabel(task.status),
    severity: "muted",
  };
}

export function unixTimeLabel(value: string): string {
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds <= 0) return "-";
  return new Date(seconds * 1000).toLocaleString();
}

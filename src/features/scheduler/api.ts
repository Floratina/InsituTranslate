import { invoke } from "@tauri-apps/api/core";

export type SchedulerAction =
  | { type: "enqueue"; taskId: string }
  | { type: "enqueueBatch"; taskIds: string[] }
  | { type: "retranslate"; taskId: string }
  | { type: "retranslateBatch"; taskIds: string[] }
  | { type: "pause"; taskId: string }
  | { type: "pauseAll" }
  | { type: "setConcurrency"; maxActiveTasks: number };

export interface SchedulerAck {
  success: boolean;
  message: string | null;
}

export interface TaskSchedulerPreferences {
  maxActiveTasks: number;
}

export function dispatchSchedulerAction(action: SchedulerAction): Promise<SchedulerAck> {
  return invoke<SchedulerAck>("dispatch_scheduler_action", { action });
}

export function getTaskSchedulerPreferences(): Promise<TaskSchedulerPreferences> {
  return invoke<TaskSchedulerPreferences>("get_task_scheduler_preferences");
}

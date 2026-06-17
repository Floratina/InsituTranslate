import { invoke } from "@tauri-apps/api/core";

import type {
  CreateTranslationTaskInput,
  TranslationConfigView,
  TranslationTaskFilters,
  TranslationTaskDetail,
  TranslationTaskView,
  UpdateTranslationConfigInput,
  UpdateTranslationTaskTagsInput,
} from "./types";

export function listTranslationTasks(
  filters?: TranslationTaskFilters,
): Promise<TranslationTaskView[]> {
  return invoke<TranslationTaskView[]>("list_translation_tasks", { filters });
}

export function createTranslationTask(
  input: CreateTranslationTaskInput,
): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("create_translation_task", { input });
}

export function startTranslationTask(id: string): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("start_translation_task", { id });
}

export function resumeTranslationTask(id: string): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("resume_translation_task", { id });
}

export function retranslateTranslationTask(id: string): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("retranslate_translation_task", { id });
}

export function deleteTranslationTask(id: string): Promise<void> {
  return invoke("delete_translation_task", { id });
}

export function updateTranslationTaskTags(
  input: UpdateTranslationTaskTagsInput,
): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("update_translation_task_tags", { input });
}

export function getTranslationTaskDetail(
  id: string,
): Promise<TranslationTaskDetail> {
  return invoke<TranslationTaskDetail>("get_translation_task_detail", { id });
}

export function getTranslationConfig(): Promise<TranslationConfigView> {
  return invoke<TranslationConfigView>("get_translation_config");
}

export function updateTranslationConfig(
  input: UpdateTranslationConfigInput,
): Promise<TranslationConfigView> {
  return invoke<TranslationConfigView>("update_translation_config", { input });
}

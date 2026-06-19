import { invoke } from "@tauri-apps/api/core";

import type {
  CreateTranslationTaskInput,
  ExportTranslationTaskInput,
  ImportTranslationTaskInput,
  TranslationConfigView,
  TranslationTaskFilters,
  TranslationTaskDetail,
  TranslationTaskIdsInput,
  TranslationTaskView,
  UpdateTranslationConfigInput,
  UpdateTranslationTaskNameInput,
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

export function pickTranslationTaskFile(): Promise<string | null> {
  return invoke<string | null>("pick_translation_task_file");
}

export function importTranslationTask(
  input: ImportTranslationTaskInput,
): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("import_translation_task", { input });
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

export function pauseTranslationTask(id: string): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("pause_translation_task", { id });
}

export function startTranslationTasksBatch(
  input: TranslationTaskIdsInput,
): Promise<TranslationTaskView[]> {
  return invoke<TranslationTaskView[]>("start_translation_tasks_batch", { input });
}

export function pauseTranslationTasksBatch(): Promise<void> {
  return invoke("pause_translation_tasks_batch");
}

export function deleteTranslationTask(id: string): Promise<void> {
  return invoke("delete_translation_task", { id });
}

export function deleteTranslationTasks(input: TranslationTaskIdsInput): Promise<void> {
  return invoke("delete_translation_tasks", { input });
}

export function updateTranslationTaskName(
  input: UpdateTranslationTaskNameInput,
): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("update_translation_task_name", { input });
}

export function updateTranslationTaskTags(
  input: UpdateTranslationTaskTagsInput,
): Promise<TranslationTaskView> {
  return invoke<TranslationTaskView>("update_translation_task_tags", { input });
}

export function openTranslationTaskFolder(id: string): Promise<void> {
  return invoke("open_translation_task_folder", { id });
}

export function exportTranslationTask(input: ExportTranslationTaskInput): Promise<void> {
  return invoke("export_translation_task", { input });
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

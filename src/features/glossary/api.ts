import { invoke } from "@tauri-apps/api/core";

import type {
  CreateGlossaryEntryInput,
  DeleteGlossaryEntryInput,
  ExportGlossaryInput,
  GlossaryEntriesQuery,
  GlossaryEntryPage,
  GlossaryFailedChunkPage,
  GlossaryFailedChunksQuery,
  GlossaryEntryView,
  GlossaryListQuery,
  GlossaryView,
  ImportGlossaryInput,
  PrepareAutoGlossaryInput,
  UpdateGlossaryInput,
  UpdateGlossaryEntryInput,
} from "./types";

export function listGlossaries(
  query?: GlossaryListQuery | null,
): Promise<GlossaryView[]> {
  return invoke<GlossaryView[]>("list_glossaries", { query });
}

export function importGlossary(
  input: ImportGlossaryInput,
): Promise<GlossaryView> {
  return invoke<GlossaryView>("import_glossary", { input });
}

export function updateGlossary(
  input: UpdateGlossaryInput,
): Promise<GlossaryView> {
  return invoke<GlossaryView>("update_glossary", { input });
}

export function deleteGlossary(id: string): Promise<void> {
  return invoke("delete_glossary", { id });
}

export function glossaryFileAvailable(id: string): Promise<boolean> {
  return invoke<boolean>("glossary_file_available", { id });
}

export function openGlossaryFolder(id: string): Promise<void> {
  return invoke("open_glossary_folder", { id });
}

export function exportGlossary(input: ExportGlossaryInput): Promise<void> {
  return invoke("export_glossary", { input });
}

export function getGlossaryEntries(
  query: GlossaryEntriesQuery,
): Promise<GlossaryEntryPage> {
  return invoke<GlossaryEntryPage>("get_glossary_entries", { query });
}

export function getGlossaryFailedChunks(
  query: GlossaryFailedChunksQuery,
): Promise<GlossaryFailedChunkPage> {
  return invoke<GlossaryFailedChunkPage>("get_glossary_failed_chunks", { query });
}

export function createGlossaryEntry(
  input: CreateGlossaryEntryInput,
): Promise<GlossaryEntryView> {
  return invoke<GlossaryEntryView>("create_glossary_entry", { input });
}

export function updateGlossaryEntry(
  input: UpdateGlossaryEntryInput,
): Promise<GlossaryEntryView> {
  return invoke<GlossaryEntryView>("update_glossary_entry", { input });
}

export function deleteGlossaryEntry(
  input: DeleteGlossaryEntryInput,
): Promise<void> {
  return invoke("delete_glossary_entry", { input });
}

export function prepareAutoGlossaryForTask(
  input: PrepareAutoGlossaryInput,
): Promise<GlossaryView | null> {
  return invoke<GlossaryView | null>("prepare_auto_glossary_for_task", { input });
}

export function pickGlossaryFile(): Promise<string | null> {
  return invoke<string | null>("pick_glossary_file");
}

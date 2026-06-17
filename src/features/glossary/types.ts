export type SortMode = "created-desc" | "created-asc" | "az";

export type GlossarySortField = "name" | "tags" | "language";

export type GlossaryEntrySortField = "src" | "dst";

export type GlossaryExportFormat = "csv" | "json";

export interface GlossarySortInput {
  field: GlossarySortField;
  mode: SortMode;
}

export interface GlossaryListQuery {
  search?: string | null;
  tag?: string | null;
  sourceLanguage?: string | null;
  targetLanguage?: string | null;
  sort?: GlossarySortInput | null;
}

export interface GlossaryView {
  id: string;
  name: string;
  ingPath: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string[];
  sourceType: string;
  entryCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface ImportGlossaryInput {
  filePath: string;
  name: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string[];
}

export interface UpdateGlossaryInput {
  glossaryId: string;
  name: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string[];
}

export interface ExportGlossaryInput {
  id: string;
  format: GlossaryExportFormat;
}

export interface GlossaryEntrySortInput {
  field: GlossaryEntrySortField;
  mode: SortMode;
}

export interface GlossaryEntriesQuery {
  id: string;
  page: number;
  pageSize: number;
  search?: string | null;
  sort?: GlossaryEntrySortInput | null;
}

export interface GlossaryEntryView {
  id: string;
  src: string;
  dst: string;
  createdAt: string;
  updatedAt: string;
}

export interface GlossaryEntryPage {
  entries: GlossaryEntryView[];
  total: number;
  page: number;
  pageSize: number;
}

export interface CreateGlossaryEntryInput {
  glossaryId: string;
  src: string;
  dst: string;
}

export interface UpdateGlossaryEntryInput {
  glossaryId: string;
  entryId: string;
  src: string;
  dst: string;
}

export interface DeleteGlossaryEntryInput {
  glossaryId: string;
  entryId: string;
}

export interface PrepareAutoGlossaryInput {
  taskId: string;
}

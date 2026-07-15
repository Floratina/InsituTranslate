export type TranslationTaskStatus =
  | "pending"
  | "queued"
  | "running"
  | "interrupted-pending"
  | "interrupted"
  | "failed"
  | "success";

export type TranslationChunkStatus =
  | "pending"
  | "interrupted"
  | "failed"
  | "success";

export interface TokenStats {
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  thinkingTokens: number;
  totalTokens: number;
}

export interface TextTokenStats {
  sourceTokens: number;
  targetTokens: number;
  totalTokens: number;
}

export type ProgressStepState = "pending" | "running" | "success" | "failed";

export interface ProgressStep {
  state: ProgressStepState;
  current: number;
  total: number;
  percent: number;
  label: string;
}

export interface ProgressDetail {
  ast: ProgressStep;
  chunking: ProgressStep;
  glossary: ProgressStep;
  translating: ProgressStep;
  restore: ProgressStep;
}

export interface StartTranslationTaskCreationResult {
  clientTaskId: string;
}

export type TranslationTaskCreationStage = "ast" | "chunking" | "glossary";
export type TranslationTaskCreationStatus =
  | "queued"
  | "running"
  | "success"
  | "failed"
  | "cancelled";

export interface TranslationTaskCreationProgressPayload {
  clientTaskId: string;
  filePath: string;
  stage: TranslationTaskCreationStage;
  step: ProgressStep;
  status: TranslationTaskCreationStatus;
  task: TranslationTaskView | null;
  error: string | null;
}

export interface TranslationConfigView {
  sourceLanguage: string;
  customSourceLanguage: string;
  targetLanguage: string;
  customTargetLanguage: string;
  providerId: string;
  modelId: string;
  assistantId: string;
  chunkTokenLimit: number;
  maxConcurrency: number;
  maxRetries: number;
  maxFailurePercentage: number;
  rateLimitStrategy: RateLimitStrategy;
  maxRequestsPerMinute: number;
  maxTokensPerMinute: number;
  contextHandlingMode: ContextHandlingMode;
  useGlobalBackground?: boolean;
  enableTranslation: boolean;
  useGlossary: boolean;
  glossaryMode: GlossaryMode;
  glossaryId: string | null;
  thinkingEffort: ThinkingEffort;
  useWebSearch: boolean;
  useCustomParameters: boolean;
  glossaryGenerationConfig: GlossaryGenerationConfig;
  confidenceMode: ConfidenceMode;
  pdfParsingMode: PdfParsingMode;
}

export type RateLimitStrategy = "dynamic" | "manual";
export type ContextHandlingMode =
  | "off"
  | "sliding-window-target"
  | "sliding-window-source"
  | "global-background";
export type GlossaryMode = "auto" | "existing";
export type ThinkingEffort =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export interface GlossaryGenerationConfig {
  providerId: string;
  modelId: string;
  assistantId: string | null;
  thinkingEffort: ThinkingEffort;
  useWebSearch: boolean;
  useCustomParameters: boolean;
  maxFailurePercentage: number;
}
export type ConfidenceMode = "off" | "confidence-index";
export type PdfParsingMode = "local-first" | "mineru-first" | "local-only" | "mineru-only";

export type UpdateTranslationConfigInput = TranslationConfigView;

export interface CreateTranslationTaskInput {
  filePath: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string[];
  providerId: string;
  modelId: string;
  assistantId: string | null;
  enableTranslation: boolean;
  useGlossary: boolean;
  glossaryMode: GlossaryMode;
  glossaryId: string | null;
  glossaryGenerationConfig: GlossaryGenerationConfig;
}

export type TaskRuntimeConfigDomain = "translation" | "glossary";
export type TaskRuntimeActionReason = "local-config-missing" | "remote-model-unavailable";

export interface TaskRuntimeActionRequired {
  taskId: string;
  domains: TaskRuntimeConfigDomain[];
  reason: TaskRuntimeActionReason;
}

export interface ReplaceTaskRuntimeSnapshotInput {
  taskId: string;
  config: TranslationConfigView;
}

export interface TranslationTaskFilters {
  tag?: string | null;
  sourceLanguage?: string | null;
  targetLanguage?: string | null;
}

export interface UpdateTranslationTaskTagsInput {
  id: string;
  tags: string[];
}

export interface ImportTranslationTaskInput {
  filePath: string;
}

export interface UpdateTranslationTaskNameInput {
  id: string;
  name: string;
}

export interface UpdateTranslationTaskInfoInput {
  id: string;
  name: string;
  tags: string[];
}

export interface TranslationTaskIdsInput {
  ids: string[];
}

export type TranslationTaskExportFormat = "source" | "pdf" | "pdf-bilingual";

export interface TranslationTaskPdfOptions {
  pageSize: string;
  margin: string;
  scale: number;
}

export interface ExportTranslationTaskInput {
  id: string;
  format: TranslationTaskExportFormat;
  outputName: string;
  pdfOptions?: TranslationTaskPdfOptions | null;
}

export interface TranslationTaskView {
  id: string;
  name: string;
  inpPath: string;
  sourcePath: string;
  sourceLanguage: string;
  targetLanguage: string;
  status: TranslationTaskStatus;
  progress: number;
  providerId: string;
  modelId: string;
  modelRequestName: string;
  assistantId: string | null;
  enableTranslation: boolean;
  glossaryId: string | null;
  tags: string[];
  totalChunks: number;
  completedChunks: number;
  failedChunks: number;
  interruptedChunks: number;
  tokenStats: TokenStats;
  textTokenStats: TextTokenStats;
  errorRate: number;
  lastError: string | null;
  rateLimitStatus: string | null;
  activeRetry: TranslationTaskActiveRetry | null;
  progressDetail: ProgressDetail | null;
  createdAt: string;
  updatedAt: string;
}

export interface TranslationTaskActiveRetry {
  current: number;
  max: number;
  message: string;
}

export interface TranslationChunkView {
  id: string;
  sequence: number;
  mapJson: string;
  preprocessedText: string;
  sourceText: string;
  afterTranslateText: string;
  translatedText: string;
  confidence: number | null;
  status: TranslationChunkStatus;
  retryCount: number;
  errorMessage: string | null;
  tokenStats: TokenStats;
  textTokenStats: TextTokenStats;
  updatedAt: string;
}

export interface TranslationTaskDetail {
  task: TranslationTaskView;
  chunks: TranslationChunkView[];
}

export interface TranslationProgressPayload {
  task: TranslationTaskView;
}

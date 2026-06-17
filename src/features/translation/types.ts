export type TranslationTaskStatus =
  | "pending"
  | "running"
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
  rateLimitStrategy: RateLimitStrategy;
  maxRequestsPerMinute: number;
  maxTokensPerMinute: number;
  useGlossary: boolean;
  glossaryMode: GlossaryMode;
  glossaryId: string | null;
}

export type RateLimitStrategy = "dynamic" | "manual";
export type GlossaryMode = "auto" | "existing";

export type UpdateTranslationConfigInput = TranslationConfigView;

export interface CreateTranslationTaskInput {
  filePath: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string[];
  providerId: string;
  modelId: string;
  assistantId: string | null;
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
  tags: string[];
  totalChunks: number;
  completedChunks: number;
  failedChunks: number;
  interruptedChunks: number;
  tokenStats: TokenStats;
  errorRate: number;
  lastError: string | null;
  rateLimitStatus: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface TranslationChunkView {
  id: string;
  sequence: number;
  sourceText: string;
  translatedText: string;
  status: TranslationChunkStatus;
  retryCount: number;
  errorMessage: string | null;
  tokenStats: TokenStats;
  updatedAt: string;
}

export interface TranslationTaskDetail {
  task: TranslationTaskView;
  chunks: TranslationChunkView[];
}

export interface TranslationProgressPayload {
  task: TranslationTaskView;
}

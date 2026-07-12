import type { AssistantView } from "@/features/assistants/types";
import type {
  GlossarySortField,
  GlossaryView,
  SortMode,
} from "@/features/glossary/types";
import type { ProviderPurpose, ProviderView } from "@/features/providers/types";
import type {
  ProgressStep,
  TranslationConfigView,
  TranslationTaskCreationStage,
  TranslationTaskCreationStatus,
  TranslationTaskDetail,
  TranslationTaskView,
} from "@/features/translation/types";

export type SessionResourceStatus = "idle" | "loading" | "ready" | "error";

export class SessionResource<T> {
  private data: T | undefined;
  private promise: Promise<T> | null = null;

  status: SessionResourceStatus = "idle";
  updatedAt = 0;
  error: unknown = null;

  read(): T | undefined {
    return this.data;
  }

  hasData(): boolean {
    return this.data !== undefined;
  }

  set(value: T): T {
    this.data = value;
    this.promise = null;
    this.status = "ready";
    this.updatedAt = Date.now();
    this.error = null;
    return value;
  }

  update(updater: (current: T | undefined) => T): T {
    return this.set(updater(this.data));
  }

  invalidate(): void {
    this.data = undefined;
    this.promise = null;
    this.status = "idle";
    this.updatedAt = 0;
    this.error = null;
  }

  loadOnce(loader: () => Promise<T>): Promise<T> {
    if (this.data !== undefined) return Promise.resolve(this.data);
    if (this.promise) return this.promise;
    return this.refresh(loader);
  }

  refresh(loader: () => Promise<T>): Promise<T> {
    this.status = "loading";
    this.error = null;
    this.promise = loader()
      .then((value) => this.set(value))
      .catch((error: unknown) => {
        this.status = "error";
        this.error = error;
        this.promise = null;
        throw error;
      });
    return this.promise;
  }
}

export interface StartPageDraft {
  filePaths: string[];
  sourceLanguage: string;
  detectedSourceLanguage: string | null;
  targetLanguage: string;
  providerId: string;
  modelId: string;
  assistantId: string;
  config: TranslationConfigView;
}

export interface StartCreationJob {
  clientTaskId: string;
  filePath: string;
  status: TranslationTaskCreationStatus;
  stages: Record<TranslationTaskCreationStage, ProgressStep>;
  taskId: string | null;
  error: string | null;
}

export interface GlossaryIndexCache {
  glossaries: GlossaryView[];
  filterSeed: GlossaryView[];
  selectedGlossaryId: string | null;
  search: string;
  tagFilter: string;
  sourceFilter: string;
  targetFilter: string;
  listSort: {
    field: GlossarySortField;
    mode: SortMode;
  };
  listPage: number;
  listPageSize: number;
  listWidths: number[];
}

type StartCreationJobListener = () => void;

class StartCreationJobStore {
  private jobs: StartCreationJob[] = [];
  private listeners = new Set<StartCreationJobListener>();

  read(): StartCreationJob[] {
    return this.jobs;
  }

  subscribe(listener: StartCreationJobListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  set(jobs: StartCreationJob[]): void {
    this.jobs = jobs;
    this.notify();
  }

  upsert(job: StartCreationJob): void {
    const index = this.jobs.findIndex(
      (item) => item.clientTaskId === job.clientTaskId || item.filePath === job.filePath,
    );
    if (index === -1) {
      this.set([...this.jobs, job]);
      return;
    }
    this.set(this.jobs.map((item, itemIndex) => (itemIndex === index ? job : item)));
  }

  update(
    clientTaskId: string,
    updater: (job: StartCreationJob) => StartCreationJob,
  ): void {
    this.set(
      this.jobs.map((job) => (job.clientTaskId === clientTaskId ? updater(job) : job)),
    );
  }

  updateByFilePath(
    filePath: string,
    updater: (job: StartCreationJob) => StartCreationJob,
  ): void {
    this.set(this.jobs.map((job) => (job.filePath === filePath ? updater(job) : job)));
  }

  remove(clientTaskId: string): void {
    this.set(this.jobs.filter((job) => job.clientTaskId !== clientTaskId));
  }

  removeByFilePath(filePath: string): void {
    this.set(this.jobs.filter((job) => job.filePath !== filePath));
  }

  private notify(): void {
    this.listeners.forEach((listener) => listener());
  }
}

const providerResources = new Map<ProviderPurpose, SessionResource<ProviderView[]>>();
const assistantResources = new Map<ProviderPurpose, SessionResource<AssistantView[]>>();
const proofreadingDetailResources = new Map<string, SessionResource<TranslationTaskDetail>>();

function resourceForPurpose<T>(
  resources: Map<ProviderPurpose, SessionResource<T>>,
  purpose: ProviderPurpose,
): SessionResource<T> {
  let resource = resources.get(purpose);
  if (!resource) {
    resource = new SessionResource<T>();
    resources.set(purpose, resource);
  }
  return resource;
}

function proofreadingDetailResource(id: string): SessionResource<TranslationTaskDetail> {
  let resource = proofreadingDetailResources.get(id);
  if (!resource) {
    resource = new SessionResource<TranslationTaskDetail>();
    proofreadingDetailResources.set(id, resource);
  }
  return resource;
}

export const appSessionCache = {
  translationConfig: new SessionResource<TranslationConfigView>(),
  startDraft: new SessionResource<StartPageDraft>(),
  startCreationJobs: new StartCreationJobStore(),
  glossaryIndex: new SessionResource<GlossaryIndexCache>(),
  proofreadingTasks: new SessionResource<TranslationTaskView[]>(),
  providerSelectedIds: new Map<ProviderPurpose, string>(),
  assistantSelectedIds: new Map<ProviderPurpose, string>(),
  proofreadingSelectedTaskId: "",

  providers(purpose: ProviderPurpose): SessionResource<ProviderView[]> {
    return resourceForPurpose(providerResources, purpose);
  },

  assistants(purpose: ProviderPurpose): SessionResource<AssistantView[]> {
    return resourceForPurpose(assistantResources, purpose);
  },

  invalidateProviders(): void {
    providerResources.forEach((resource) => resource.invalidate());
  },

  proofreadingDetail(id: string): SessionResource<TranslationTaskDetail> {
    return proofreadingDetailResource(id);
  },

  invalidateProofreading(): void {
    this.proofreadingTasks.invalidate();
    this.proofreadingSelectedTaskId = "";
    proofreadingDetailResources.clear();
  },
};

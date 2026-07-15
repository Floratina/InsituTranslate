import { useCallback, useRef, useState } from "react";
import { MotionConfig } from "motion/react";

import { AppShell, type AppPage } from "@/components/layout/AppShell";
import { ToastProvider } from "@/components/ui/toast-stack";
import { AppearanceProvider } from "@/features/appearance/AppearanceProvider";
import { PRIMARY_PAGE_FADE_UP_STYLE } from "@/lib/motion";
import { appSessionCache } from "@/lib/session-cache";
import ProviderSettingsPage from "@/views/ProviderSettingsPage";
import AppearanceSettingsPage from "@/views/AppearanceSettingsPage";
import AssistantSettingsPage, {
  type AssistantNavigationGuard,
} from "@/views/AssistantSettingsPage";
import GlossaryPage from "@/views/GlossaryPage";
import ProofreadingPage from "@/views/ProofreadingPage";
import StartPage from "@/views/StartPage";
import TranslationTasksPage from "@/views/TranslationTasksPage";

function App() {
  const [activePage, setActivePage] = useState<AppPage>("start");
  const assistantNavigationGuardRef = useRef<AssistantNavigationGuard | null>(null);

  const registerAssistantNavigationGuard = useCallback(
    (guard: AssistantNavigationGuard | null): void => {
      assistantNavigationGuardRef.current = guard;
    },
    [],
  );

  function navigate(page: AppPage): void {
    if (page === activePage) return;
    if (activePage === "assistants" && assistantNavigationGuardRef.current) {
      assistantNavigationGuardRef.current(() => setActivePage(page));
      return;
    }
    setActivePage(page);
  }

  const pageContent =
    activePage === "start" ? (
      <StartPage onTaskCreated={() => navigate("tasks")} />
    ) : activePage === "tasks" ? (
      <TranslationTasksPage
        onOpenProofreading={(taskId) => {
          appSessionCache.proofreadingSelectedTaskId = taskId;
          navigate("proofreading");
        }}
        onOpenGlossary={(glossaryId) => {
          appSessionCache.glossaryNavigationTargetId = glossaryId;
          appSessionCache.glossaryIndex.invalidate();
          navigate("glossary");
        }}
      />
    ) : activePage === "proofreading" ? (
      <ProofreadingPage />
    ) : activePage === "glossary" ? (
      <GlossaryPage />
    ) : activePage === "providers" ? (
      <ProviderSettingsPage />
    ) : activePage === "assistants" ? (
      <AssistantSettingsPage
        onRegisterNavigationGuard={registerAssistantNavigationGuard}
      />
    ) : (
      <AppearanceSettingsPage />
    );

  return (
    <MotionConfig reducedMotion="user">
      <AppearanceProvider>
        <ToastProvider>
          <AppShell activePage={activePage} onNavigate={navigate}>
            <div
              key={activePage}
              style={PRIMARY_PAGE_FADE_UP_STYLE}
              className="app-fade-up-enter flex min-h-0 min-w-0 flex-1 overflow-hidden"
            >
              {pageContent}
            </div>
          </AppShell>
        </ToastProvider>
      </AppearanceProvider>
    </MotionConfig>
  );
}

export default App;

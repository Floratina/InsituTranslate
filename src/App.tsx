import { useCallback, useRef, useState } from "react";
import { motion, MotionConfig } from "motion/react";

import { AppShell, type AppPage } from "@/components/layout/AppShell";
import { ToastProvider } from "@/components/ui/toast-stack";
import { AppearanceProvider } from "@/features/appearance/AppearanceProvider";
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

const pageTransition = {
  duration: 0.20,
  ease: [0.03, 0.59, 0.19, 1] as const,
};

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
    <MotionConfig
      reducedMotion="user"
      transition={{ duration: 0.20, ease: [0.03, 0.59, 0.19, 1] }}
    >
      <AppearanceProvider>
        <ToastProvider>
          <AppShell activePage={activePage} onNavigate={navigate}>
            <motion.div
              key={activePage}
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              transition={pageTransition}
              className="flex min-h-0 min-w-0 flex-1 overflow-hidden"
            >
              {pageContent}
            </motion.div>
          </AppShell>
        </ToastProvider>
      </AppearanceProvider>
    </MotionConfig>
  );
}

export default App;

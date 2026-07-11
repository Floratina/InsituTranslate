import {
  createContext,
  forwardRef,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { CheckCircle2, CircleAlert, Info, TriangleAlert } from "lucide-react";
import { AnimatePresence, motion, type PanInfo } from "motion/react";

import { cn } from "@/lib/utils";

export type ToastVariant = "default" | "error" | "success" | "warning";
type ToastDismissDirection = -1 | 1;

export interface ToastMessage {
  id: number;
  message: string;
  variant: ToastVariant;
}

const motionEase = [0.03, 0.59, 0.19, 1] as const;
const toastExitOffset = 40;

const toastTransition = {
  x: { duration: 0.2, ease: motionEase },
  layout: { duration: 0.2, ease: motionEase },
  opacity: { duration: 0.2, ease: motionEase },
};

const toastVariants = {
  initial: { opacity: 0, x: 40 },
  animate: { opacity: 1, x: 0 },
  exit: (exitX: number) => ({
    opacity: 0,
    x: exitX,
  }),
};

const swipeDismissThreshold = 72;

interface ToastContextValue {
  pushToast: (message: string, variant?: ToastVariant) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

async function writeClipboardText(message: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(message);
    return;
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = message;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();
    const copied = document.execCommand("copy");
    textarea.remove();
    if (!copied) throw new Error("Unable to copy notification");
  }
}

function useToastStackState() {
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  const nextId = useRef(0);
  const dismissTimers = useRef<Map<number, number>>(new Map());

  const dismissToast = useCallback((id: number): void => {
    const timer = dismissTimers.current.get(id);
    if (timer !== undefined) window.clearTimeout(timer);
    dismissTimers.current.delete(id);
    setToasts((items) => items.filter((item) => item.id !== id));
  }, []);

  const pushToast = useCallback(
    (message: string, variant: ToastVariant = "default"): void => {
      if (!message.trim()) return;
      const id = ++nextId.current;
      setToasts((items) => [...items, { id, message, variant }].slice(-6));
      dismissTimers.current.set(
        id,
        window.setTimeout(() => dismissToast(id), 7000),
      );
    },
    [dismissToast],
  );

  useEffect(
    () => () => {
      dismissTimers.current.forEach((timer) => window.clearTimeout(timer));
      dismissTimers.current.clear();
    },
    [],
  );

  return { toasts, pushToast, dismissToast };
}

export function useToast(): ToastContextValue {
  const context = useContext(ToastContext);
  if (!context) {
    throw new Error("useToast must be used within ToastProvider");
  }
  return context;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const { toasts, pushToast, dismissToast } = useToastStackState();
  return (
    <ToastContext.Provider value={{ pushToast }}>
      {children}
      <ToastStack toasts={toasts} onDismiss={dismissToast} onPush={pushToast} />
    </ToastContext.Provider>
  );
}

interface ToastStackProps {
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
  onPush: (message: string, variant?: ToastVariant) => void;
}

interface ToastBubbleProps {
  toast: ToastMessage;
  onDismiss: (id: number) => void;
  onCopy: (toast: ToastMessage) => void;
}

const ToastBubble = forwardRef<HTMLButtonElement, ToastBubbleProps>(function ToastBubble(
  { toast, onDismiss, onCopy },
  ref,
) {
  const [exitX, setExitX] = useState(toastExitOffset);
  const dragged = useRef(false);

  const Icon =
    toast.variant === "error"
      ? CircleAlert
      : toast.variant === "success"
        ? CheckCircle2
        : toast.variant === "warning"
          ? TriangleAlert
          : Info;

  function finishSwipe(_: PointerEvent, info: PanInfo): void {
    const distance = info.offset.x;
    const shouldDismiss =
      Math.abs(distance) >= swipeDismissThreshold ||
      Math.abs(info.velocity.x) >= 650;
    if (shouldDismiss) {
      const direction: ToastDismissDirection =
        distance < 0 || info.velocity.x < 0 ? -1 : 1;
      setExitX(distance + direction * toastExitOffset);
      window.setTimeout(() => onDismiss(toast.id), 0);
      return;
    }
    window.setTimeout(() => {
      dragged.current = false;
    }, 0);
  }

  return (
    <motion.button
      layout="position"
      key={toast.id}
      type="button"
      drag="x"
      dragConstraints={{ left: 0, right: 0 }}
      dragElastic={0.32}
      dragMomentum={false}
      ref={ref}
      custom={exitX}
      variants={toastVariants}
      initial="initial"
      animate="animate"
      exit="exit"
      transition={toastTransition}
      onDragStart={() => {
        dragged.current = true;
      }}
      onDragEnd={finishSwipe}
      onClick={() => {
        if (!dragged.current) onCopy(toast);
        dragged.current = false;
      }}
      data-toast-id={toast.id}
      className={cn(
        "pointer-events-auto flex min-h-10 w-full transform-gpu cursor-default items-center gap-2 rounded-[6px] border bg-clip-padding px-3 py-2 text-left text-xs backdrop-blur-xl backdrop-saturate-150",
        "shadow-[0_8px_24px_rgba(15,23,42,0.18),inset_0_1px_0_rgba(255,255,255,0.42)] dark:shadow-[0_10px_28px_rgba(0,0,0,0.36),inset_0_1px_0_rgba(255,255,255,0.08)]",
        "focus-visible:ring-3 focus-visible:ring-ring/35 focus-visible:outline-none",
        toast.variant === "default" &&
          "!border-sky-300/80 bg-sky-50/74 text-sky-900 hover:bg-sky-100/84 dark:!border-sky-700/80 dark:bg-[#08283a]/56 dark:text-sky-100 dark:hover:bg-[#0b344b]/66",
        toast.variant === "error" &&
          "!border-red-300/80 bg-red-50/74 text-red-800 hover:bg-red-100/84 dark:!border-red-700/80 dark:bg-[#3a1016]/56 dark:text-red-200 dark:hover:bg-[#48151d]/66",
        toast.variant === "success" &&
          "!border-emerald-300/80 bg-emerald-50/74 text-emerald-800 hover:bg-emerald-100/84 dark:!border-emerald-700/80 dark:bg-[#0b3025]/56 dark:text-emerald-200 dark:hover:bg-[#0f3d2f]/66",
        toast.variant === "warning" &&
          "!border-amber-400/80 bg-amber-50/74 text-amber-900 hover:bg-amber-100/84 dark:!border-amber-700/80 dark:bg-[#3a2a08]/56 dark:text-amber-100 dark:hover:bg-[#47340b]/66",
      )}
    >
      <Icon className="size-3.5 shrink-0" strokeWidth={1.8} />
      <span className="min-w-0 flex-1 break-words leading-5">{toast.message}</span>
    </motion.button>
  );
});

export function ToastStack({ toasts, onDismiss, onPush }: ToastStackProps) {
  function copyToast(toast: ToastMessage): void {
    void writeClipboardText(toast.message)
      .then(() => onPush("复制成功", "success"))
      .catch(() => onPush("复制失败", "error"));
  }

  return createPortal(
    <div
      data-testid="toast-stack"
      className="pointer-events-none fixed top-11 right-3 z-[9999] flex w-[min(360px,calc(100vw-1.5rem))] flex-col gap-2"
    >
      <AnimatePresence initial={false} mode="popLayout">
        {toasts.map((toast) => (
          <ToastBubble
            key={toast.id}
            toast={toast}
            onDismiss={onDismiss}
            onCopy={copyToast}
          />
        ))}
      </AnimatePresence>
    </div>,
    document.body,
  );
}

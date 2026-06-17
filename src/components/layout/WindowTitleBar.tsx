import { useEffect, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";

import { Button } from "@/components/ui/button";

const minimumWindowSize = new LogicalSize(920, 620);

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export function WindowTitleBar() {
  const [maximized, setMaximized] = useState<boolean>(false);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void appWindow.setMinSize(minimumWindowSize).catch(() => undefined);
    void appWindow.isMaximized().then((value) => {
      if (!disposed) setMaximized(value);
    });
    void appWindow.onResized(() => {
      void appWindow.isMaximized().then((value) => {
        if (!disposed) setMaximized(value);
      });
    }).then((dispose) => {
      unlisten = dispose;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  async function toggleMaximize(): Promise<void> {
    if (!isTauriRuntime()) return;
    const appWindow = getCurrentWindow();
    await appWindow.toggleMaximize();
    setMaximized(await appWindow.isMaximized());
  }

  return (
    <header className="flex h-8 shrink-0 select-none items-stretch bg-background text-foreground">
      <div data-tauri-drag-region className="min-w-0 flex-1" />
      <div className="flex items-stretch">
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title="最小化"
          aria-label="最小化"
          className="h-8 w-10 rounded-none border-0! focus-visible:border-0! focus-visible:ring-0"
          onClick={() => {
            if (isTauriRuntime()) void getCurrentWindow().minimize();
          }}
        >
          <Minus className="size-3.5" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title={maximized ? "还原" : "最大化"}
          aria-label={maximized ? "还原" : "最大化"}
          className="h-8 w-10 rounded-none border-0! focus-visible:border-0! focus-visible:ring-0"
          onClick={() => void toggleMaximize()}
        >
          {maximized ? <Copy className="size-3.5 scale-x-[-1]" /> : <Square className="size-3" />}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title="关闭"
          aria-label="关闭"
          className="h-8 w-11 rounded-none border-0! hover:bg-red-600 hover:text-white focus-visible:border-0! focus-visible:ring-0 dark:hover:bg-red-600 dark:hover:text-white"
          onClick={() => {
            if (isTauriRuntime()) void getCurrentWindow().close();
          }}
        >
          <X className="size-3.5" />
        </Button>
      </div>
    </header>
  );
}

import * as React from "react";
import { Popover as PopoverPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

interface PopoverControlContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
}

const PopoverControlContext = React.createContext<PopoverControlContextValue | null>(null);

function Popover({
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Root>) {
  const [uncontrolledOpen, setUncontrolledOpen] = React.useState(defaultOpen ?? false);
  const open = openProp ?? uncontrolledOpen;

  const setOpen = React.useCallback(
    (nextOpen: boolean) => {
      if (openProp === undefined) {
        setUncontrolledOpen(nextOpen);
      }
      onOpenChange?.(nextOpen);
    },
    [onOpenChange, openProp],
  );

  return (
    <PopoverControlContext.Provider value={{ open, setOpen }}>
      <PopoverPrimitive.Root open={open} onOpenChange={setOpen} {...props} />
    </PopoverControlContext.Provider>
  );
}

const PopoverTrigger = React.forwardRef<
  React.ElementRef<typeof PopoverPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof PopoverPrimitive.Trigger>
>(
  (
    {
      disabled,
      onClick,
      onPointerCancel,
      onPointerDown,
      onPointerUp,
      ...props
    },
    ref,
  ) => {
    const control = React.useContext(PopoverControlContext);
    const pendingPointerRef = React.useRef(false);
    const pendingPointerIdRef = React.useRef<number | null>(null);
    const suppressNextClickRef = React.useRef(false);

    const clearPendingPointer = React.useCallback((target?: HTMLButtonElement) => {
      if (
        target &&
        pendingPointerIdRef.current !== null &&
        target.hasPointerCapture(pendingPointerIdRef.current)
      ) {
        target.releasePointerCapture(pendingPointerIdRef.current);
      }
      pendingPointerRef.current = false;
      pendingPointerIdRef.current = null;
    }, []);

    const handlePointerDown: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      onPointerDown?.(event);

      if (
        event.defaultPrevented ||
        disabled ||
        control === null ||
        event.button !== 0 ||
        event.ctrlKey
      ) {
        return;
      }

      pendingPointerRef.current = true;
      pendingPointerIdRef.current = event.pointerId;
      suppressNextClickRef.current = true;
      event.currentTarget.setPointerCapture(event.pointerId);
      event.preventDefault();
    };

    const handlePointerUp: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      onPointerUp?.(event);

      if (event.defaultPrevented || disabled || control === null || !pendingPointerRef.current) {
        return;
      }

      const rect = event.currentTarget.getBoundingClientRect();
      const releasedInside =
        event.clientX >= rect.left &&
        event.clientX <= rect.right &&
        event.clientY >= rect.top &&
        event.clientY <= rect.bottom;

      clearPendingPointer(event.currentTarget);
      if (releasedInside) {
        control.setOpen(!control.open);
      }
    };

    const handlePointerCancel: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      clearPendingPointer(event.currentTarget);
      onPointerCancel?.(event);
    };

    const handleClick: React.MouseEventHandler<HTMLButtonElement> = (event) => {
      onClick?.(event);

      if (!suppressNextClickRef.current) {
        return;
      }

      suppressNextClickRef.current = false;
      event.preventDefault();
      event.stopPropagation();
    };

    return (
      <PopoverPrimitive.Trigger
        {...props}
        ref={ref}
        disabled={disabled}
        onClick={handleClick}
        onPointerCancel={handlePointerCancel}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
      />
    );
  },
);
PopoverTrigger.displayName = "PopoverTrigger";

function PopoverContent({
  className,
  align = "center",
  sideOffset = 6,
  children,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content>) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content align={align} sideOffset={sideOffset} asChild {...props}>
        <div
          style={{
            transformOrigin: "var(--radix-popover-content-transform-origin)",
          }}
          className={cn(
            "floating-menu-enter z-[70] w-80 overflow-hidden rounded-[6px] border bg-popover p-2 text-popover-foreground shadow-lg outline-none transform-gpu",
            className,
          )}
        >
          {children}
        </div>
      </PopoverPrimitive.Content>
    </PopoverPrimitive.Portal>
  );
}

export { Popover, PopoverTrigger, PopoverContent };

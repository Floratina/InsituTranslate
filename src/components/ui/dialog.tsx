import * as React from "react"
import { X } from "lucide-react"
import { Dialog as DialogPrimitive } from "radix-ui"
import { AnimatePresence, motion } from "motion/react"

import { cn } from "@/lib/utils"

const Dialog = DialogPrimitive.Root
const DialogTrigger = DialogPrimitive.Trigger
const DialogClose = DialogPrimitive.Close

function DialogContent({
  className,
  children,
  open,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  open?: boolean
}) {
  if (open !== undefined) {
    return (
      <DialogPrimitive.Portal forceMount>
        <AnimatePresence>
          {open && (
            <>
              <DialogPrimitive.Overlay asChild forceMount>
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: 0.20, ease: [0.03, 0.59, 0.19, 1] }}
                  className="fixed inset-0 z-50 bg-black/35"
                />
              </DialogPrimitive.Overlay>
              <DialogPrimitive.Content asChild forceMount {...props}>
                <motion.div
                  data-slot="dialog-content"
                  initial={{ opacity: 0, y: 0, scale: 0.975 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  exit={{ opacity: 0, y: 0, scale: 0.985 }}
                  transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
                  className={cn(
                    "fixed top-1/2 left-1/2 z-50 w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 outline-none",
                    "scrollbar-subtle grid max-h-[85vh] gap-4 overflow-y-auto rounded-[6px] border bg-background p-4 shadow-xl",
                    className,
                  )}
                >
                  {children}
                  <DialogPrimitive.Close className="absolute top-2 right-2 inline-flex size-7 items-center justify-center rounded-[6px] text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground">
                    <X className="size-4" strokeWidth={1.8} />
                    <span className="sr-only">关闭</span>
                  </DialogPrimitive.Close>
                </motion.div>
              </DialogPrimitive.Content>
            </>
          )}
        </AnimatePresence>
      </DialogPrimitive.Portal>
    )
  }

  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay asChild>
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
          className="fixed inset-0 z-50 bg-black/35"
        />
      </DialogPrimitive.Overlay>
      <DialogPrimitive.Content
        className="fixed top-1/2 left-1/2 z-50 w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 outline-none"
        {...props}
      >
        <motion.div
          data-slot="dialog-content"
          initial={{ opacity: 0, y: 8, scale: 0.975 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ duration: 0.25, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "scrollbar-subtle relative grid max-h-[85vh] gap-4 overflow-y-auto rounded-[6px] border bg-background p-4 shadow-xl",
            className,
          )}
        >
          {children}
          <DialogPrimitive.Close className="absolute top-2 right-2 inline-flex size-7 items-center justify-center rounded-[6px] text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground">
            <X className="size-4" strokeWidth={1.8} />
            <span className="sr-only">关闭</span>
          </DialogPrimitive.Close>
        </motion.div>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("grid gap-1.5", className)}
      {...props}
    />
  )
}

function DialogField({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-field"
      className={cn("grid gap-2", className)}
      {...props}
    />
  )
}

function DialogFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn("flex items-center justify-end gap-3", className)}
      {...props}
    />
  )
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("pr-8 text-base font-semibold", className)}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogTrigger,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogField,
  DialogFooter,
  DialogTitle,
  DialogDescription,
}

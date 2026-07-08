import * as React from "react"
import { AlertDialog as AlertDialogPrimitive } from "radix-ui"
import { AnimatePresence, motion } from "motion/react"

import { cn } from "@/lib/utils"

const AlertDialog = AlertDialogPrimitive.Root
const AlertDialogTrigger = AlertDialogPrimitive.Trigger
const AlertDialogCancel = AlertDialogPrimitive.Cancel
const AlertDialogAction = AlertDialogPrimitive.Action

function AlertDialogContent({
  className,
  children,
  open,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Content> & {
  open?: boolean
}) {
  if (open !== undefined) {
    return (
      <AlertDialogPrimitive.Portal forceMount>
        <AnimatePresence>
          {open && (
            <>
              <AlertDialogPrimitive.Overlay asChild forceMount>
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: 0.20, ease: [0.03, 0.59, 0.19, 1] }}
                  className="fixed inset-0 z-50 bg-black/35"
                />
              </AlertDialogPrimitive.Overlay>
              <AlertDialogPrimitive.Content asChild forceMount {...props}>
                <motion.div
                  initial={{ opacity: 0, y: 0, scale: 0.975 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  exit={{ opacity: 0, y: 0, scale: 0.985 }}
                  transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
                  className={cn(
                    "fixed top-1/2 left-1/2 z-50 w-[calc(100%-2rem)] max-w-sm -translate-x-1/2 -translate-y-1/2 outline-none",
                    "grid gap-4 rounded-[12px] border bg-popover p-4 shadow-xl",
                    className,
                  )}
                >
                  {children}
                </motion.div>
              </AlertDialogPrimitive.Content>
            </>
          )}
        </AnimatePresence>
      </AlertDialogPrimitive.Portal>
    )
  }

  return (
    <AlertDialogPrimitive.Portal>
      <AlertDialogPrimitive.Overlay asChild>
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.20, ease: [0.03, 0.59, 0.19, 1] }}
          className="fixed inset-0 z-50 bg-black/35"
        />
      </AlertDialogPrimitive.Overlay>
      <AlertDialogPrimitive.Content
        className="fixed top-1/2 left-1/2 z-50 w-[calc(100%-2rem)] max-w-sm -translate-x-1/2 -translate-y-1/2 outline-none"
        {...props}
      >
        <motion.div
          initial={{ opacity: 0, y: 8, scale: 0.975 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ duration: 0.25, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "grid gap-4 rounded-[12px] border bg-popover p-4 shadow-xl",
            className,
          )}
        >
          {children}
        </motion.div>
      </AlertDialogPrimitive.Content>
    </AlertDialogPrimitive.Portal>
  )
}

function AlertDialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Title>) {
  return (
    <AlertDialogPrimitive.Title
      data-slot="alert-dialog-title"
      className={cn("text-base font-semibold", className)}
      {...props}
    />
  )
}

function AlertDialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Description>) {
  return (
    <AlertDialogPrimitive.Description
      data-slot="alert-dialog-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  AlertDialog,
  AlertDialogTrigger,
  AlertDialogCancel,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogTitle,
  AlertDialogDescription,
}

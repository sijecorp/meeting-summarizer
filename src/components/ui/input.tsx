import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-control border border-input bg-surface px-3 py-1 text-body text-ink transition-colors file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-label file:text-ink placeholder:text-ink-2 disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-sunken disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20",
        className
      )}
      {...props}
    />
  )
}

export { Input }

import * as React from "react"
import { motion } from "motion/react"
import { Switch as SwitchPrimitive } from "radix-ui"
import { spring } from "@/lib/motion"
import { cn } from "@/lib/utils"

// Controlled only (`checked`). The knob springs across (design-system 5: `spring.snappy`), except
// when a key toggled it: keyboard actions do not move.
function Switch({
  className,
  onKeyDown,
  onClick,
  ...props
}: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  const byKey = React.useRef(false)
  const [instant, setInstant] = React.useState(false)
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-transparent transition-colors after:absolute after:-inset-x-3 after:-inset-y-2 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 data-checked:bg-primary data-unchecked:bg-input data-disabled:cursor-not-allowed data-disabled:opacity-50",
        className
      )}
      onKeyDown={(e) => {
        if (e.key === " " || e.key === "Enter") byKey.current = true
        onKeyDown?.(e)
      }}
      onClick={(e) => {
        setInstant(byKey.current)
        byKey.current = false
        onClick?.(e)
      }}
      {...props}
    >
      <SwitchPrimitive.Thumb asChild>
        {/* 16px in a 36px track with a 1px border: 1px off, 17px on. */}
        <motion.span
          data-slot="switch-thumb"
          initial={false}
          animate={{ x: props.checked ? 17 : 1 }}
          transition={instant ? { duration: 0 } : spring.snappy}
          className="pointer-events-none block size-4 rounded-full bg-surface shadow-sm"
        />
      </SwitchPrimitive.Thumb>
    </SwitchPrimitive.Root>
  )
}

export { Switch }

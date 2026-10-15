import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const badgeVariants = cva(
  "group/badge inline-flex h-6 w-fit shrink-0 items-center justify-center gap-1.5 overflow-hidden rounded-full border border-transparent px-2.5 text-label whitespace-nowrap transition-colors aria-invalid:border-destructive aria-invalid:ring-destructive/20 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&>svg]:pointer-events-none [&>svg]:size-3!",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a]:hover:bg-accent-press",
        secondary: "bg-sunken text-ink [a]:hover:bg-line",
        // Like Alert destructive: danger-line text would fail 4.5:1, so ink text on danger-bg.
        destructive:
          "border-danger-line bg-danger-bg text-ink [&>svg]:text-danger-line [a]:hover:bg-[color-mix(in_oklch,var(--color-danger-bg),var(--color-danger-line)_12%)]",
        outline: "border-line-strong text-ink [a]:hover:bg-sunken",
        ghost: "text-ink hover:bg-sunken",
        link: "text-primary underline-offset-4 hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }

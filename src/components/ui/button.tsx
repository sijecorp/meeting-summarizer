import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const buttonVariants = cva(
  // Focus: the global 2px ring (theme.css). Press: a 120ms shrink, transform only.
  "group/button inline-flex shrink-0 items-center justify-center rounded-control border border-transparent bg-clip-padding text-body font-semibold whitespace-nowrap select-none motion-safe:transition-transform motion-safe:duration-120 motion-safe:ease-out motion-safe:active:not-aria-[haspopup]:scale-[0.97] disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        // White text: 5.3:1 on brand, 7.4:1 on accent-press.
        default: "bg-primary text-primary-foreground hover:bg-accent-press",
        // 녹음 시작 / 종료: white on ink.
        ink: "bg-ink text-white hover:bg-ink/85",
        outline:
          "border-line-strong bg-transparent text-ink hover:bg-sunken aria-expanded:bg-sunken",
        ghost: "text-ink hover:bg-sunken aria-expanded:bg-sunken",
        // Design-system 3: danger-line text would fail 4.5:1, so ink text on danger-bg.
        destructive:
          "border-danger-line bg-danger-bg text-ink hover:bg-[color-mix(in_oklch,var(--color-danger-bg),var(--color-danger-line)_12%)] [&_svg]:text-danger-line",
        // Brand text is 4.8:1 on paper.
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default:
          "h-9 gap-1.5 px-3.5 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        xs: "h-6 gap-1 rounded-md px-2 text-label has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 gap-1 px-3 text-label has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-10 gap-2 px-4 has-data-[icon=inline-end]:pr-3.5 has-data-[icon=inline-start]:pl-3.5",
        icon: "size-9",
        "icon-xs": "size-6 rounded-md [&_svg:not([class*='size-'])]:size-3.5",
        "icon-sm": "size-8",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }

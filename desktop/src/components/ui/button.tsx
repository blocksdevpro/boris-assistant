import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center whitespace-nowrap rounded-[10px] border border-transparent bg-clip-padding text-[13px] font-semibold tracking-[-0.01em] outline-none select-none transition-[background-color,border-color,color,box-shadow,transform,opacity] duration-200 ease-[var(--boris-ease-standard)] focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/30 active:not-aria-[haspopup]:scale-[0.975] disabled:pointer-events-none disabled:opacity-45 aria-invalid:border-destructive aria-invalid:ring-[3px] aria-invalid:ring-destructive/20 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-[inset_0_0.5px_0_rgba(255,255,255,0.32),0_1px_2px_rgba(0,0,0,0.16)] hover:bg-[color-mix(in_oklch,var(--primary),white_8%)]",
        outline:
          "border-border bg-background/65 text-foreground shadow-[inset_0_0.5px_0_rgba(255,255,255,0.08)] backdrop-blur-xl hover:bg-muted aria-expanded:bg-muted dark:border-white/10 dark:bg-white/[0.055] dark:hover:bg-white/[0.09]",
        secondary:
          "bg-secondary text-secondary-foreground shadow-[inset_0_0.5px_0_rgba(255,255,255,0.08)] hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_7%)] aria-expanded:bg-secondary",
        ghost:
          "text-muted-foreground hover:bg-muted hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground dark:hover:bg-white/[0.07]",
        destructive:
          "bg-destructive text-white shadow-[inset_0_0.5px_0_rgba(255,255,255,0.25),0_1px_2px_rgba(0,0,0,0.18)] hover:bg-[color-mix(in_oklch,var(--destructive),white_8%)] focus-visible:border-destructive/70 focus-visible:ring-destructive/30",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default:
          "h-9 gap-1.5 px-3.5 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        xs: "h-7 gap-1 rounded-lg px-2 text-[11px] in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 gap-1 rounded-[9px] px-3 text-xs in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-10 gap-2 rounded-xl px-4 text-sm has-data-[icon=inline-end]:pr-3.5 has-data-[icon=inline-start]:pl-3.5",
        icon: "size-9",
        "icon-xs":
          "size-6 rounded-[min(var(--radius-md),10px)] in-data-[slot=button-group]:rounded-lg [&_svg:not([class*='size-'])]:size-3",
        "icon-sm":
          "size-7 rounded-[min(var(--radius-md),12px)] in-data-[slot=button-group]:rounded-lg",
        "icon-lg": "size-9",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

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
  const Comp = asChild ? Slot.Root : "button";

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants };

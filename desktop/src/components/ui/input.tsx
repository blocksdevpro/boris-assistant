import * as React from "react";

import { cn } from "@/lib/utils";

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-[10px] border border-input bg-background/55 px-3 py-1 text-base text-foreground shadow-[inset_0_1px_2px_rgba(0,0,0,0.08),inset_0_0.5px_0_rgba(255,255,255,0.07)] outline-none backdrop-blur-xl transition-[background-color,border-color,box-shadow,opacity] duration-200 ease-[var(--boris-ease-standard)] file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground/75 focus-visible:border-ring focus-visible:bg-background/75 focus-visible:ring-[3px] focus-visible:ring-ring/25 disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-muted/60 disabled:opacity-45 aria-invalid:border-destructive aria-invalid:ring-[3px] aria-invalid:ring-destructive/20 md:text-[13px] dark:bg-white/[0.055] dark:focus-visible:bg-white/[0.075] dark:disabled:bg-white/[0.035]",
        className,
      )}
      {...props}
    />
  );
}

export { Input };

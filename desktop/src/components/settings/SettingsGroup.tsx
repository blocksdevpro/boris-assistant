import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** Inset grouped list — header outside the card, Apple Settings style. */
export function SettingsGroup({
  title,
  footer,
  children,
  className,
}: {
  title?: string;
  footer?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("flex flex-col gap-2", className)}>
      {title ? (
        <h3 className="px-4 text-[13px] font-normal leading-none text-white/45">
          {title}
        </h3>
      ) : null}
      <div className="settings-group overflow-hidden rounded-[12px]">
        {children}
      </div>
      {footer ? (
        <p className="px-4 text-[12px] leading-snug text-white/35">{footer}</p>
      ) : null}
    </section>
  );
}

export function SettingsRow({
  label,
  subtitle,
  children,
  last,
  /** Stack control under the label (full width) instead of trailing. */
  stacked,
}: {
  label: string;
  subtitle?: string;
  children?: ReactNode;
  last?: boolean;
  stacked?: boolean;
}) {
  if (stacked) {
    return (
      <div
        className={cn(
          "flex flex-col gap-2 px-4 py-3",
          !last && "border-b border-white/[0.06]",
        )}
      >
        <div className="min-w-0">
          <p className="text-[15px] font-normal leading-snug tracking-[-0.01em] text-white/[0.92]">
            {label}
          </p>
          {subtitle ? (
            <p className="mt-0.5 text-[12px] leading-snug text-white/35">
              {subtitle}
            </p>
          ) : null}
        </div>
        {children}
      </div>
    );
  }

  return (
    <div
      className={cn(
        "flex min-h-[48px] items-center gap-3 px-4 py-2.5",
        !last && "border-b border-white/[0.06]",
      )}
    >
      <div className="min-w-0 flex-1 pr-2">
        <p className="text-[15px] font-normal leading-snug tracking-[-0.01em] text-white/[0.92]">
          {label}
        </p>
        {subtitle ? (
          <p className="mt-0.5 text-[12px] leading-snug text-white/35">
            {subtitle}
          </p>
        ) : null}
      </div>
      {children ? (
        <div className="flex min-w-0 max-w-[min(100%,18rem)] shrink-0 items-center justify-end gap-2">
          {children}
        </div>
      ) : null}
    </div>
  );
}

/** Full-width field under a label (API key, log filter). */
export function SettingsField({
  label,
  subtitle,
  last,
  children,
}: {
  label: string;
  subtitle?: string;
  last?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex flex-col gap-2 px-4 py-3",
        !last && "border-b border-white/[0.06]",
      )}
    >
      <div className="min-w-0">
        <p className="text-[13px] font-normal text-white/45">{label}</p>
        {subtitle ? (
          <p className="mt-0.5 text-[12px] leading-snug text-white/35">{subtitle}</p>
        ) : null}
      </div>
      {children}
    </div>
  );
}

import { useId, type ReactNode } from "react";
import { cn } from "@/lib/utils";

/** Inset grouped list — header outside the card, Apple Settings style. */
export function SettingsGroup({
  title,
  footer,
  children,
  className,
  id,
  action,
}: {
  title?: string;
  footer?: ReactNode;
  children: ReactNode;
  className?: string;
  id?: string;
  action?: ReactNode;
}) {
  const generatedId = useId();
  const headingId = title ? `${id ?? generatedId}-heading` : undefined;
  const footerId = footer ? `${id ?? generatedId}-footer` : undefined;

  return (
    <section
      id={id}
      aria-labelledby={headingId}
      aria-describedby={footerId}
      className={cn(
        "settings-section flex scroll-mt-4 flex-col gap-2.5",
        className,
      )}
    >
      {title ? (
        <div className="settings-section__header flex min-h-7 items-end justify-between gap-3 px-3">
          <h2
            id={headingId}
            className="text-[12px] font-medium leading-none tracking-[0.01em] text-white/48"
          >
            {title}
          </h2>
          {action}
        </div>
      ) : null}
      <div className="settings-group settings-section__surface overflow-hidden rounded-[16px] border border-white/[0.055] shadow-[0_1px_0_rgba(255,255,255,0.025)_inset,0_14px_36px_rgba(0,0,0,0.10)]">
        {children}
      </div>
      {footer ? (
        <p
          id={footerId}
          className="settings-section__footer px-3 text-[12px] leading-[1.45] text-white/38"
        >
          {footer}
        </p>
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
  labelFor,
}: {
  label: string;
  subtitle?: string;
  children?: ReactNode;
  last?: boolean;
  stacked?: boolean;
  labelFor?: string;
}) {
  const Label = labelFor ? "label" : "p";
  if (stacked) {
    return (
      <div
        data-settings-row
        className={cn(
          "settings-row flex flex-col gap-2.5 px-4 py-3.5 transition-colors duration-150",
          !last && "border-b border-white/[0.06]",
        )}
      >
        <div className="min-w-0">
          <Label
            htmlFor={labelFor}
            className="text-[14px] font-medium leading-snug tracking-[-0.01em] text-white/[0.9]"
          >
            {label}
          </Label>
          {subtitle ? (
            <p className="mt-1 text-[12px] leading-snug text-white/38">
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
      data-settings-row
      className={cn(
        "settings-row flex min-h-[56px] items-center gap-3 px-4 py-2.5 transition-colors duration-150",
        !last && "border-b border-white/[0.06]",
      )}
    >
      <div className="min-w-0 flex-1 pr-2">
        <Label
          htmlFor={labelFor}
          className="text-[14px] font-medium leading-snug tracking-[-0.01em] text-white/[0.9]"
        >
          {label}
        </Label>
        {subtitle ? (
          <p className="mt-1 text-[12px] leading-snug text-white/38">
            {subtitle}
          </p>
        ) : null}
      </div>
      {children ? (
        <div className="settings-row__control flex min-w-0 max-w-[min(55%,18rem)] shrink-0 items-center justify-end gap-2">
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
  labelFor,
}: {
  label: string;
  subtitle?: string;
  last?: boolean;
  children: ReactNode;
  labelFor?: string;
}) {
  return (
    <div
      data-settings-row
      className={cn(
        "settings-row settings-field flex flex-col gap-2.5 px-4 py-3.5",
        !last && "border-b border-white/[0.06]",
      )}
    >
      <div className="min-w-0">
        {labelFor ? (
          <label
            htmlFor={labelFor}
            className="text-[13px] font-medium text-white/58"
          >
            {label}
          </label>
        ) : (
          <p className="text-[13px] font-medium text-white/58">{label}</p>
        )}
        {subtitle ? (
          <p className="mt-1 text-[12px] leading-snug text-white/38">
            {subtitle}
          </p>
        ) : null}
      </div>
      {children}
    </div>
  );
}

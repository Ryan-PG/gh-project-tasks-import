import type { ReactNode } from "react";

/** Small presentational primitives, kept in one file so the panels stay readable. */

type ButtonProps = {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  variant?: "primary" | "secondary" | "ghost" | "danger";
  type?: "button" | "submit";
  title?: string;
};

export function Button({
  children,
  onClick,
  disabled,
  variant = "secondary",
  type = "button",
  title,
}: ButtonProps) {
  const base =
    "inline-flex items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50";
  const styles: Record<string, string> = {
    primary:
      "bg-[var(--color-accent)] text-[var(--color-accent-ink)] hover:brightness-110",
    secondary:
      "border border-[var(--color-edge)] bg-[var(--color-surface-raised)] hover:bg-[var(--color-surface-sunken)]",
    ghost: "hover:bg-[var(--color-surface-sunken)]",
    danger:
      "border border-red-500/40 text-red-600 hover:bg-red-500/10 dark:text-red-400",
  };
  return (
    <button
      type={type}
      title={title}
      onClick={onClick}
      disabled={disabled}
      className={`${base} ${styles[variant]}`}
    >
      {children}
    </button>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="block space-y-1.5">
      <span className="block text-sm font-medium">{label}</span>
      {children}
      {hint ? (
        <span className="block text-xs text-[var(--color-ink-muted)]">{hint}</span>
      ) : null}
    </label>
  );
}

export function TextInput({
  value,
  onChange,
  placeholder,
  type = "text",
  disabled,
  spellCheck,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
  disabled?: boolean;
  spellCheck?: boolean;
}) {
  return (
    <input
      type={type}
      value={value}
      disabled={disabled}
      spellCheck={spellCheck}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      className="mono w-full rounded-lg border border-[var(--color-edge)] bg-[var(--color-surface-raised)] px-3 py-2 text-sm disabled:opacity-50"
    />
  );
}

export function Checkbox({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2.5">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
      />
      <span>
        <span className="block text-sm">{label}</span>
        {hint ? (
          <span className="block text-xs text-[var(--color-ink-muted)]">{hint}</span>
        ) : null}
      </span>
    </label>
  );
}

export function Badge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "good" | "warn" | "bad" | "accent";
}) {
  const tones: Record<string, string> = {
    neutral:
      "border-[var(--color-edge)] bg-[var(--color-surface-sunken)] text-[var(--color-ink-muted)]",
    good: "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400",
    warn: "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-400",
    bad: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-400",
    accent:
      "border-[var(--color-accent)]/30 bg-[var(--color-accent)]/10 text-[var(--color-accent)]",
  };
  return (
    <span
      className={`inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium ${tones[tone]}`}
    >
      {children}
    </span>
  );
}

export function Card({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={`card p-5 ${className}`}>{children}</div>;
}

export function SectionTitle({
  title,
  subtitle,
  right,
}: {
  title: string;
  subtitle?: string;
  right?: ReactNode;
}) {
  return (
    <div className="mb-4 flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">{title}</h2>
        {subtitle ? (
          <p className="mt-0.5 text-sm text-[var(--color-ink-muted)]">{subtitle}</p>
        ) : null}
      </div>
      {right}
    </div>
  );
}

export function Alert({
  tone,
  title,
  children,
}: {
  tone: "info" | "good" | "warn" | "bad";
  title?: string;
  children: ReactNode;
}) {
  const tones: Record<string, string> = {
    info: "border-[var(--color-edge)] bg-[var(--color-surface-sunken)]",
    good: "border-emerald-500/30 bg-emerald-500/10",
    warn: "border-amber-500/30 bg-amber-500/10",
    bad: "border-red-500/30 bg-red-500/10",
  };
  return (
    <div className={`rounded-lg border px-3.5 py-3 text-sm ${tones[tone]}`}>
      {title ? <div className="mb-1 font-medium">{title}</div> : null}
      <div className="text-[var(--color-ink-muted)]">{children}</div>
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <span className="inline-flex items-center gap-2 text-sm text-[var(--color-ink-muted)]">
      <span
        aria-hidden
        className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-[var(--color-edge)] border-t-[var(--color-accent)]"
      />
      {label}
    </span>
  );
}

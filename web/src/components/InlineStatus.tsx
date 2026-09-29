import type { ReactNode } from "react";

/** Empty, loading and error states are a quiet inline line, never a hero. */
export function InlineStatus({
  children,
  tone = "muted",
  action,
  role,
}: {
  children: ReactNode;
  tone?: "muted" | "error";
  action?: { label: string; onClick: () => void };
  role?: "status" | "alert";
}) {
  return (
    <p className={`inline-status inline-status--${tone}`} role={role}>
      <span>{children}</span>
      {action ? (
        <button type="button" className="link-button" onClick={action.onClick}>
          {action.label}
        </button>
      ) : null}
    </p>
  );
}

"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

import { cx } from "./ui";

/**
 * A button that asks before it acts: the first press opens an inline question with the
 * consequence spelled out; only the second button runs `onConfirm`. The question closes when
 * `onConfirm` resolves and stays open when it rejects (the caller shows the error). Focus moves
 * into the question and back to the trigger when it is dismissed.
 */
export function ConfirmAction({
  trigger,
  question,
  confirmLabel,
  onConfirm,
  pending = false,
  tone = "danger",
  triggerClassName,
  children,
}: {
  trigger: ReactNode;
  question: ReactNode;
  confirmLabel: string;
  onConfirm: () => Promise<unknown>;
  pending?: boolean;
  tone?: "danger" | "primary";
  triggerClassName?: string;
  /** Extra content inside the question, e.g. a warning. */
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(false);

  useEffect(() => {
    if (open) confirmRef.current?.focus();
    else if (wasOpen.current) triggerRef.current?.focus();
    wasOpen.current = open;
  }, [open]);

  if (!open) {
    return (
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen(true)}
        className={triggerClassName ?? "btn btn-secondary btn-sm self-start"}
      >
        {trigger}
      </button>
    );
  }

  return (
    <div
      role="group"
      aria-label={typeof trigger === "string" ? trigger : undefined}
      className="flex w-full flex-col gap-3 rounded-lg border border-line bg-surface-2 p-3.5 text-sm"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !pending) setOpen(false);
      }}
    >
      <div className="text-ink">{question}</div>
      {children}
      <div className="flex flex-wrap gap-2">
        <button
          ref={confirmRef}
          type="button"
          onClick={() => {
            onConfirm().then(
              () => setOpen(false),
              () => undefined,
            );
          }}
          disabled={pending}
          className={cx(
            "btn btn-sm",
            tone === "danger" ? "btn-danger" : "btn-primary",
          )}
        >
          {pending ? "Working…" : confirmLabel}
        </button>
        <button
          type="button"
          onClick={() => setOpen(false)}
          disabled={pending}
          className="btn btn-ghost btn-sm"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

"use client";

import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import { confirmsPhrase } from "~/lib/data-controls";

/**
 * A button for something that can't be undone: the first press opens the consequence and a field
 * where the person types `phrase`; only then does the confirm button run `onConfirm`. It closes
 * when `onConfirm` resolves and stays open when it rejects (the caller shows the error). Focus
 * moves into the field, and back to the trigger when it is dismissed.
 */
export function TypedConfirmAction({
  trigger,
  phrase,
  confirmLabel,
  onConfirm,
  pending = false,
  children,
}: {
  trigger: string;
  /** What to type, in lowercase; case and surrounding spaces don't matter. */
  phrase: string;
  confirmLabel: string;
  onConfirm: () => Promise<unknown>;
  pending?: boolean;
  /** The consequence, spelled out. */
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(false);
  const inputId = useId();
  const confirmed = confirmsPhrase(typed, phrase);

  useEffect(() => {
    if (open) inputRef.current?.focus();
    else if (wasOpen.current) triggerRef.current?.focus();
    wasOpen.current = open;
  }, [open]);

  const close = () => {
    setOpen(false);
    setTyped("");
  };

  if (!open) {
    return (
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen(true)}
        className="btn btn-secondary btn-sm self-start"
      >
        {trigger}
      </button>
    );
  }

  return (
    <form
      aria-label={trigger}
      className="flex w-full flex-col gap-3 rounded-lg border border-line bg-surface-2 p-3.5 text-sm"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !pending) close();
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (!confirmed || pending) return;
        onConfirm().then(close, () => undefined);
      }}
    >
      <div className="flex flex-col gap-2 text-ink-2">{children}</div>
      <div className="flex flex-col gap-1.5">
        <label htmlFor={inputId} className="font-medium text-ink">
          Type <span className="font-mono">{phrase}</span> to confirm
        </label>
        <input
          ref={inputRef}
          id={inputId}
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={pending}
          className="field font-mono sm:max-w-72"
        />
      </div>
      <div className="flex flex-wrap gap-2">
        <button
          type="submit"
          disabled={!confirmed || pending}
          className="btn btn-danger btn-sm"
        >
          {pending ? "Working…" : confirmLabel}
        </button>
        <button
          type="button"
          onClick={close}
          disabled={pending}
          className="btn btn-ghost btn-sm"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

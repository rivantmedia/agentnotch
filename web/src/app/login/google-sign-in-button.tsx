"use client";

import { useState } from "react";

import { createClient } from "~/lib/supabase/client";

/** Starts Supabase's Google sign-in; Supabase sends the browser back to /auth/callback. */
export function GoogleSignInButton({ next }: { next: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function signIn() {
    setBusy(true);
    setError(null);
    try {
      const callback = new URL("/auth/callback", window.location.origin);
      callback.searchParams.set("next", next);
      const { error } = await createClient().auth.signInWithOAuth({
        provider: "google",
        options: { redirectTo: callback.toString() },
      });
      // On success the browser is already navigating to Google.
      if (error) {
        setError(`Couldn't start the sign-in: ${error.message}`);
        setBusy(false);
      }
    } catch {
      setError(
        "Couldn't start the sign-in. Check your connection and try again.",
      );
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-2">
      <button
        type="button"
        onClick={() => void signIn()}
        disabled={busy}
        aria-busy={busy || undefined}
        className="btn btn-secondary min-h-11 w-full gap-3 text-[0.9375rem]"
      >
        <GoogleG />
        {busy ? "Opening Google…" : "Continue with Google"}
      </button>
      <p role="alert" className="text-sm text-critical-ink empty:hidden">
        {error}
      </p>
    </div>
  );
}

/** Google's "G" mark, as its sign-in buttons show it. */
function GoogleG() {
  return (
    <svg viewBox="0 0 18 18" aria-hidden="true" className="size-4.5">
      <path
        fill="#4285F4"
        d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.7-1.57 2.68-3.88 2.68-6.62Z"
      />
      <path
        fill="#34A853"
        d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.8.54-1.84.86-3.04.86-2.34 0-4.33-1.58-5.04-3.7H.96v2.33A9 9 0 0 0 9 18Z"
      />
      <path
        fill="#FBBC05"
        d="M3.96 10.72A5.41 5.41 0 0 1 3.68 9c0-.6.1-1.18.28-1.72V4.95H.96A9 9 0 0 0 0 9c0 1.45.35 2.83.96 4.05l3-2.33Z"
      />
      <path
        fill="#EA4335"
        d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58A8.97 8.97 0 0 0 9 0 9 9 0 0 0 .96 4.95l3 2.33C4.67 5.16 6.66 3.58 9 3.58Z"
      />
    </svg>
  );
}

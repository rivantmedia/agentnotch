"use client";

import { QueryErrorResetBoundary } from "@tanstack/react-query";
import { Component, Suspense, type ReactNode } from "react";

import { ErrorNotice } from "./ui";

/**
 * Where a page section's data loads. Its children read queries with `useSuspenseQuery`, which
 * the page prefetched on the server: the server streams the finished section, and the browser
 * hydrates the same data, so both render the same HTML. While loading, `fallback` shows; if a
 * query fails, the section shows what failed and a retry, and the rest of the page stays.
 */
export function QueryBoundary({
  what,
  fallback,
  resetKeys,
  children,
}: {
  /** What the section loads, for the error message: "your accounts". */
  what: string;
  fallback: ReactNode;
  /**
   * What the children load by, e.g. a period picked outside the section: when one changes after
   * a failure, the section tries again with it instead of keeping the old error.
   */
  resetKeys?: readonly unknown[];
  children: ReactNode;
}) {
  return (
    <QueryErrorResetBoundary>
      {({ reset }) => (
        <ErrorBoundary
          onReset={reset}
          resetKeys={resetKeys}
          fallback={(error, retry) => (
            <ErrorNotice error={error} what={what} onRetry={retry} />
          )}
        >
          <Suspense fallback={fallback}>{children}</Suspense>
        </ErrorBoundary>
      )}
    </QueryErrorResetBoundary>
  );
}

type BoundaryProps = {
  onReset: () => void;
  resetKeys?: readonly unknown[];
  fallback: (error: unknown, retry: () => void) => ReactNode;
  children: ReactNode;
};

type BoundaryState = { failed: boolean; error: unknown };

class ErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false, error: null };

  static getDerivedStateFromError(error: unknown): BoundaryState {
    return { failed: true, error };
  }

  componentDidUpdate(previous: BoundaryProps) {
    if (
      this.state.failed &&
      changed(previous.resetKeys, this.props.resetKeys)
    ) {
      this.retry();
    }
  }

  retry = () => {
    // Clears the failed queries so they fetch again when the children mount.
    this.props.onReset();
    this.setState({ failed: false, error: null });
  };

  render() {
    if (this.state.failed) {
      return this.props.fallback(this.state.error, this.retry);
    }
    return this.props.children;
  }
}

function changed(
  before: readonly unknown[] | undefined,
  after: readonly unknown[] | undefined,
): boolean {
  if (before === after) return false;
  if (before === undefined || after === undefined) return true;
  if (before.length !== after.length) return true;
  return before.some((value, i) => !Object.is(value, after[i]));
}

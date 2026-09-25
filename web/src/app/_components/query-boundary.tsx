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
  children,
}: {
  /** What the section loads, for the error message: "your accounts". */
  what: string;
  fallback: ReactNode;
  children: ReactNode;
}) {
  return (
    <QueryErrorResetBoundary>
      {({ reset }) => (
        <ErrorBoundary
          onReset={reset}
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
  fallback: (error: unknown, retry: () => void) => ReactNode;
  children: ReactNode;
};

type BoundaryState = { failed: boolean; error: unknown };

class ErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false, error: null };

  static getDerivedStateFromError(error: unknown): BoundaryState {
    return { failed: true, error };
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

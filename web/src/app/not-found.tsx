import Link from "next/link";

export default function NotFound() {
  return (
    <div className="mx-auto flex max-w-xl flex-col items-start gap-4 px-4 py-16 sm:py-24">
      <p className="text-sm font-medium text-ink-2">404</p>
      <h1 className="text-2xl font-semibold tracking-tight">Nothing here</h1>
      <p className="text-ink-2">
        This page doesn&apos;t exist, or it belongs to an account you can&apos;t
        see. Accounts shared through a pool appear after you join it.
      </p>
      <div className="flex flex-wrap gap-2">
        <Link href="/dashboard" className="btn btn-primary">
          Go to the dashboard
        </Link>
        <Link href="/pools" className="btn btn-secondary">
          Join a pool
        </Link>
      </div>
    </div>
  );
}

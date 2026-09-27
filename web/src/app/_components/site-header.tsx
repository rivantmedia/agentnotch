import Link from "next/link";
import { unstable_rethrow } from "next/navigation";

import { api } from "~/trpc/server";

import { LogoMark } from "./logo";
import { NavLinks } from "./nav-links";

/**
 * The bar on every page: the name, the signed-in navigation, the download page, and sign in /
 * sign out.
 */
export async function SiteHeader() {
  let viewer: Awaited<ReturnType<typeof api.viewer.current>> = null;
  try {
    viewer = await api.viewer.current();
  } catch (error) {
    // Next's own control flow (dynamic rendering, redirects) must pass through.
    unstable_rethrow(error);
    // Anything else: the page below reports the failure; the header still renders, signed out.
  }

  return (
    <header className="sticky top-0 z-30 border-b border-line bg-page/90 backdrop-blur supports-[backdrop-filter]:bg-page/75">
      <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-x-6 gap-y-2 px-4 py-2.5 sm:flex-nowrap">
        <Link
          href={viewer ? "/dashboard" : "/"}
          className="flex shrink-0 items-center gap-2 rounded-md font-semibold tracking-tight whitespace-nowrap"
        >
          <LogoMark />
          <span>Agent Notch</span>
        </Link>

        <div className="order-3 w-full sm:order-none sm:w-auto">
          {viewer ? <NavLinks /> : null}
        </div>

        {/* From md, where the email shows, min-w-0 lets it truncate to make room; otherwise the
            cluster keeps its full width and pushes Sign out off a 768–853px screen. Not below
            md: with no email to give way, the buttons would spill past the edge instead. */}
        <div className="ml-auto flex items-center gap-3 md:min-w-0">
          {/* Not in NavLinks: signed-out visitors, who need it most, don't get those. */}
          <Link href="/download" className="btn btn-ghost btn-sm">
            Download
          </Link>
          {viewer ? (
            <>
              <span
                className="hidden max-w-56 truncate text-sm text-ink-2 md:inline"
                title={viewer.email}
              >
                {viewer.email}
              </span>
              <form action="/auth/signout" method="post">
                <button type="submit" className="btn btn-ghost btn-sm">
                  Sign out
                </button>
              </form>
            </>
          ) : (
            <Link href="/login" className="btn btn-secondary btn-sm">
              Sign in
            </Link>
          )}
        </div>
      </div>
    </header>
  );
}

import "~/styles/globals.css";

import { type Metadata, type Viewport } from "next";
import Link from "next/link";

import { TRPCReactProvider } from "~/trpc/react";

import { SiteHeader } from "./_components/site-header";

export const metadata: Metadata = {
  title: { default: "Agent Notch", template: "%s · Agent Notch" },
  description:
    "Claude usage across accounts, projects and sessions, synced from the Agent Notch Mac app.",
  icons: [{ rel: "icon", url: "/favicon.ico" }],
};

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#f9f9f7" },
    { media: "(prefers-color-scheme: dark)", color: "#0d0d0d" },
  ],
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className="flex min-h-dvh flex-col">
        <a
          href="#main"
          className="sr-only z-50 rounded-md bg-surface text-sm font-medium shadow-md focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:px-3 focus:py-2"
        >
          Skip to content
        </a>
        <TRPCReactProvider>
          <SiteHeader />
          <main id="main" className="flex-1">
            {children}
          </main>
          <footer className="border-t border-line">
            <div className="mx-auto flex max-w-6xl flex-col gap-2 px-4 py-6 text-sm text-ink-2 sm:flex-row sm:items-center sm:justify-between">
              <p>
                Agent Notch syncs names and numbers only: no file paths, no
                prompts.
              </p>
              <Link href="/#privacy" className="link">
                What the app sends
              </Link>
            </div>
          </footer>
        </TRPCReactProvider>
      </body>
    </html>
  );
}

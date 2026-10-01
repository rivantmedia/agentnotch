"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cx } from "./ui";

const LINKS = [
  { href: "/dashboard", label: "Dashboard", also: ["/accounts"] },
  { href: "/usage", label: "Usage", also: [] },
  { href: "/projects", label: "Projects", also: [] },
  { href: "/pools", label: "Pools", also: [] },
  { href: "/settings", label: "Settings", also: [] },
] as const;

function isCurrent(pathname: string, prefix: string): boolean {
  return pathname === prefix || pathname.startsWith(`${prefix}/`);
}

/** The signed-in navigation; the current section is marked for sight and for screen readers. */
export function NavLinks() {
  const pathname = usePathname();
  return (
    <nav
      aria-label="Main"
      className="-m-1 flex items-center gap-1 overflow-x-auto p-1"
    >
      {LINKS.map((link) => {
        const current =
          isCurrent(pathname, link.href) ||
          link.also.some((prefix) => isCurrent(pathname, prefix));
        return (
          <Link
            key={link.href}
            href={link.href}
            aria-current={current ? "page" : undefined}
            className={cx(
              "rounded-md px-2.5 py-1.5 text-sm font-medium whitespace-nowrap transition-colors",
              current
                ? "bg-surface-2 text-ink"
                : "text-ink-2 hover:bg-surface-2 hover:text-ink",
            )}
          >
            {link.label}
          </Link>
        );
      })}
    </nav>
  );
}

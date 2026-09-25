/** The mark: a notch with two usage rings. Decorative; the name always sits beside it. */
export function LogoMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      className={className ?? "size-6"}
    >
      <rect x="1" y="3" width="22" height="18" rx="5" fill="var(--color-ink)" />
      <circle
        cx="12"
        cy="12"
        r="5.25"
        fill="none"
        stroke="var(--color-page)"
        strokeOpacity="0.35"
        strokeWidth="1.75"
      />
      <path
        d="M12 6.75a5.25 5.25 0 0 1 5.07 6.61"
        fill="none"
        stroke="var(--color-series)"
        strokeWidth="1.75"
        strokeLinecap="round"
      />
      <circle
        cx="12"
        cy="12"
        r="2.5"
        fill="none"
        stroke="var(--color-page)"
        strokeOpacity="0.35"
        strokeWidth="1.5"
      />
      <path
        d="M12 9.5a2.5 2.5 0 0 1 2.5 2.5"
        fill="none"
        stroke="var(--color-page)"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}

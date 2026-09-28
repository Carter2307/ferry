/** The Ferry mark: same paths and colors as the dashboard's and the docs' logo. */
export function FerryMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" className={className}>
      <path d="M4 13.5h16l-2.2 4.2a2 2 0 0 1-1.77 1.07H7.97A2 2 0 0 1 6.2 17.7L4 13.5Z" fill="var(--primary-solid)" />
      <path
        d="M7.5 13.5V9.25c0-.69.56-1.25 1.25-1.25h6.5c.69 0 1.25.56 1.25 1.25v4.25"
        stroke="var(--primary-solid)"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
      <path d="M12 8V4.5" stroke="var(--primary-solid)" strokeWidth="1.8" strokeLinecap="round" />
      <path
        d="M3 21.25c1.5 0 1.5-.9 3-.9s1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9"
        stroke="var(--primary-bright)"
        strokeWidth="1.5"
        strokeLinecap="round"
        opacity=".55"
      />
    </svg>
  )
}

export function Wordmark() {
  return (
    <span className="flex items-center gap-2">
      <FerryMark className="size-7" />
      <span className="font-display text-[1.3125rem] font-bold tracking-[-0.02em]">Ferry</span>
    </span>
  )
}

import * as React from 'react'
import { CalendarClock, Cog, Database, DatabaseZap, Globe, Lock, PanelsTopLeft, type LucideProps } from 'lucide-react'

import type { DatastoreKind, GitProvider, ServiceType } from '@/lib/api/types'
import { cn } from '@/lib/utils'

/** Ferry mark: a hull on a wave, in the brand green. */
export function FerryLogo({ className, ...props }: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" className={cn('size-[18px]', className)} {...props}>
      <path d="M4 13.5h16l-2.2 4.2a2 2 0 0 1-1.77 1.07H7.97A2 2 0 0 1 6.2 17.7L4 13.5Z" fill="var(--primary-solid)" />
      <path d="M7.5 13.5V9.25c0-.69.56-1.25 1.25-1.25h6.5c.69 0 1.25.56 1.25 1.25v4.25" stroke="var(--primary-solid)" strokeWidth="1.8" strokeLinejoin="round" />
      <path d="M12 8V4.5" stroke="var(--primary-solid)" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M3 21.25c1.5 0 1.5-.9 3-.9s1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9" stroke="var(--primary-bright)" strokeWidth="1.5" strokeLinecap="round" opacity=".55" />
    </svg>
  )
}

const SERVICE_ICONS: Record<ServiceType, React.ComponentType<LucideProps>> = {
  web_service: Globe,
  private_service: Lock,
  background_worker: Cog,
  cron_job: CalendarClock,
  static_site: PanelsTopLeft,
}

export function ServiceTypeIcon({ type, ...props }: { type: ServiceType } & LucideProps) {
  const Icon = SERVICE_ICONS[type]
  return <Icon aria-hidden="true" {...props} />
}

export function DatastoreKindIcon({ kind, ...props }: { kind: DatastoreKind } & LucideProps) {
  const Icon = kind === 'postgres' ? Database : DatabaseZap
  return <Icon aria-hidden="true" {...props} />
}

/** The providers' marks, in the current text color (lucide has no brand icons). */
const GIT_PROVIDER_MARKS: Record<GitProvider, { viewBox: string; d: string }> = {
  github: {
    viewBox: '0 0 16 16',
    d: 'M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z',
  },
  gitlab: {
    viewBox: '0 0 24 24',
    d: 'm23.6 9.59-.03-.09L20.3.98a.85.85 0 0 0-.34-.4.87.87 0 0 0-1 .05.87.87 0 0 0-.29.44l-2.2 6.75H7.54L5.33 1.07a.86.86 0 0 0-.29-.44.87.87 0 0 0-1-.05.86.86 0 0 0-.34.4L.43 9.5l-.03.09a6.07 6.07 0 0 0 2.01 7.01l.01.01.03.02 4.98 3.73 2.46 1.86 1.5 1.13a1.01 1.01 0 0 0 1.22 0l1.5-1.13 2.46-1.86 5.01-3.75.01-.01a6.07 6.07 0 0 0 2.01-7.01Z',
  },
}

export function GitProviderIcon({
  provider,
  className,
  ...props
}: { provider: GitProvider } & React.SVGProps<SVGSVGElement>) {
  const mark = GIT_PROVIDER_MARKS[provider]
  return (
    <svg viewBox={mark.viewBox} fill="currentColor" aria-hidden="true" className={cn('size-4', className)} {...props}>
      <path d={mark.d} />
    </svg>
  )
}

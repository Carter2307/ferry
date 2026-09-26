import * as React from 'react'
import { CalendarClock, Cog, Database, DatabaseZap, Globe, Lock, PanelsTopLeft, type LucideProps } from 'lucide-react'

import type { DatastoreKind, ServiceType } from '@/lib/api/types'
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

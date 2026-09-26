import { Check, Loader2, X } from 'lucide-react'

import { DatastoreKindIcon } from '@/components/patterns/icons'
import { CardStatusLine } from '@/components/patterns/ResourceCard'
import type { DatastoreKind, DatastoreStatus } from '@/lib/api/types'
import { cn } from '@/lib/utils'

const STATUS_LINE: Record<DatastoreStatus, { text: string; ring: string; Icon: typeof Check }> = {
  creating: { text: 'Provisioning…', ring: 'border-info-border text-info', Icon: Loader2 },
  available: { text: 'Available', ring: 'border-primary/40 text-primary', Icon: Check },
  failed: { text: 'Provisioning failed', ring: 'border-destructive-border text-destructive', Icon: X },
}

/** Card footer status (`(✓) Available`), see {@link CardStatusLine}. */
export function DatastoreStatusLine({ status, className }: { status: DatastoreStatus; className?: string }) {
  const { text, ring, Icon } = STATUS_LINE[status]
  return (
    <CardStatusLine icon={<Icon />} ring={ring} spin={status === 'creating'} className={className}>
      {text}
    </CardStatusLine>
  )
}

/** Outlined square with the kind icon (card header / page title). */
export function KindIconBox({
  kind,
  size = 'md',
  className,
}: {
  kind: DatastoreKind
  size?: 'md' | 'lg'
  className?: string
}) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'flex shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light shadow-card',
        size === 'md' ? 'size-9 [&_svg]:size-[18px]' : 'size-11 rounded-lg [&_svg]:size-5',
        className,
      )}
    >
      <DatastoreKindIcon kind={kind} strokeWidth={1.6} />
    </span>
  )
}

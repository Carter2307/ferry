import * as React from 'react'
import { AlertTriangle, Check, CircleDashed, Loader2, Pause, X } from 'lucide-react'

import type { DatastoreStatus, DeployStatus, JobStatus, ServiceState } from '@/lib/api/types'
import {
  DEPLOY_STATUS_LABELS,
  JOB_STATUS_LABELS,
  SERVICE_STATE_LABELS,
} from '@/lib/format'
import { cn } from '@/lib/utils'

import { CardStatusLine } from './ResourceCard'
import {
  DATASTORE_STATUS_TONE,
  DEPLOY_STATUS_TONE,
  JOB_STATUS_TONE,
  SERVICE_STATE_TONE,
  type StatusTone,
} from './status-tones'

const pillTone: Record<StatusTone, string> = {
  success: 'border-success/30 bg-success-soft text-success',
  warning: 'border-warning-border bg-warning-soft text-warning',
  destructive: 'border-destructive-border bg-destructive-soft text-destructive',
  info: 'border-info-border bg-info-soft text-info',
  neutral: 'border-border-strong bg-surface-200 text-foreground-light',
}

const dotTone: Record<StatusTone, string> = {
  success: 'bg-brand',
  warning: 'bg-warning',
  destructive: 'bg-destructive-solid',
  info: 'bg-info',
  neutral: 'bg-foreground-muted',
}

/** 6–8px status dot; `pulse` for in-progress states. */
export function StatusDot({ tone, pulse = false, className }: { tone: StatusTone; pulse?: boolean; className?: string }) {
  return (
    <span aria-hidden="true" className={cn('relative inline-flex size-1.5 shrink-0', className)}>
      {pulse && <span className={cn('absolute inset-0 animate-ping rounded-full opacity-60', dotTone[tone])} />}
      <span className={cn('relative inline-flex size-full rounded-full', dotTone[tone])} />
    </span>
  )
}

interface StatePillProps {
  tone: StatusTone
  label: React.ReactNode
  pulse?: boolean
  dot?: boolean
  className?: string
  title?: string
}

/** Uppercase pill with a status dot: `● LIVE`. */
export function StatePill({ tone, label, pulse = false, dot = true, className, title }: StatePillProps) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex h-5 w-fit shrink-0 items-center gap-1.5 rounded-full border px-2 text-[10.5px] leading-none font-medium tracking-[0.04em] whitespace-nowrap uppercase',
        pillTone[tone],
        className,
      )}
    >
      {dot && <StatusDot tone={tone} pulse={pulse} />}
      {label}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Domain mappings
// ---------------------------------------------------------------------------

const DATASTORE_STATUS_LABELS: Record<DatastoreStatus, string> = {
  creating: 'Creating',
  available: 'Available',
  failed: 'Failed',
}

export function ServiceStatePill({ state, className }: { state: ServiceState; className?: string }) {
  const t = SERVICE_STATE_TONE[state]
  return <StatePill tone={t.tone} pulse={t.pulse} label={SERVICE_STATE_LABELS[state]} className={className} />
}

export function DeployStatusBadge({ status, className }: { status: DeployStatus; className?: string }) {
  const t = DEPLOY_STATUS_TONE[status]
  return <StatePill tone={t.tone} pulse={t.pulse} label={DEPLOY_STATUS_LABELS[status]} className={className} />
}

export function JobStatusBadge({ status, className }: { status: JobStatus; className?: string }) {
  const t = JOB_STATUS_TONE[status]
  return <StatePill tone={t.tone} pulse={t.pulse} label={JOB_STATUS_LABELS[status]} className={className} />
}

export function DatastoreStatusBadge({ status, className }: { status: DatastoreStatus; className?: string }) {
  const t = DATASTORE_STATUS_TONE[status]
  return <StatePill tone={t.tone} pulse={t.pulse} label={DATASTORE_STATUS_LABELS[status]} className={className} />
}

const stateIconTone: Record<StatusTone, string> = {
  success: 'border-success/40 text-success',
  warning: 'border-warning-border text-warning',
  destructive: 'border-destructive-border text-destructive',
  info: 'border-info-border text-info',
  neutral: 'border-border-stronger text-foreground-lighter',
}

/**
 * Small circular outlined status icon + label, as in the Studio project card
 * footer (`(⏸) Project is paused`).
 */
export function ServiceStateLine({ state, className }: { state: ServiceState; className?: string }) {
  const { tone } = SERVICE_STATE_TONE[state]
  const Icon = {
    live: Check,
    deploying: Loader2,
    failed: X,
    degraded: AlertTriangle,
    suspended: Pause,
    not_deployed: CircleDashed,
  }[state]
  const text = {
    live: 'Service is live',
    deploying: 'Deploying…',
    failed: 'Deploy failed',
    degraded: 'Service is degraded',
    suspended: 'Service is suspended',
    not_deployed: 'Not deployed yet',
  }[state]
  return (
    <CardStatusLine icon={<Icon />} ring={stateIconTone[tone]} spin={state === 'deploying'} className={className}>
      {text}
    </CardStatusLine>
  )
}


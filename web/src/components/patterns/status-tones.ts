import type { DatastoreStatus, DeployStatus, JobStatus, ServiceState } from '@/lib/api/types'

export type StatusTone = 'success' | 'warning' | 'destructive' | 'info' | 'neutral'

export const SERVICE_STATE_TONE: Record<ServiceState, { tone: StatusTone; pulse: boolean }> = {
  live: { tone: 'success', pulse: false },
  deploying: { tone: 'info', pulse: true },
  failed: { tone: 'destructive', pulse: false },
  degraded: { tone: 'warning', pulse: false },
  suspended: { tone: 'neutral', pulse: false },
  not_deployed: { tone: 'neutral', pulse: false },
}

export const DEPLOY_STATUS_TONE: Record<DeployStatus, { tone: StatusTone; pulse: boolean }> = {
  queued: { tone: 'neutral', pulse: true },
  building: { tone: 'info', pulse: true },
  deploying: { tone: 'info', pulse: true },
  live: { tone: 'success', pulse: false },
  deactivated: { tone: 'neutral', pulse: false },
  build_failed: { tone: 'destructive', pulse: false },
  deploy_failed: { tone: 'destructive', pulse: false },
  canceled: { tone: 'neutral', pulse: false },
}

export const JOB_STATUS_TONE: Record<JobStatus, { tone: StatusTone; pulse: boolean }> = {
  pending: { tone: 'neutral', pulse: true },
  running: { tone: 'info', pulse: true },
  succeeded: { tone: 'success', pulse: false },
  failed: { tone: 'destructive', pulse: false },
  canceled: { tone: 'neutral', pulse: false },
}

export const DATASTORE_STATUS_TONE: Record<DatastoreStatus, { tone: StatusTone; pulse: boolean }> = {
  creating: { tone: 'info', pulse: true },
  available: { tone: 'success', pulse: false },
  failed: { tone: 'destructive', pulse: false },
}

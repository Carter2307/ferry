import type { JobRun } from '@/lib/api/types'
import { durationBetween } from '@/lib/format'

/** Run time: started → finished (or → now while running). */
export function jobDuration(job: Pick<JobRun, 'started_at' | 'finished_at'>, now: number): string {
  if (!job.started_at) return '—'
  return durationBetween(job.started_at, job.finished_at, now)
}

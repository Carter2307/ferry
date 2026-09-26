import { useOutletContext } from 'react-router'

import type { ServiceView } from '@/lib/api/types'

export interface ServiceOutletContext {
  /** The loaded service (always present inside service pages). */
  service: ServiceView
  /** The `:name` route param (use it as `ref` for queries/mutations). */
  name: string
}

/** Inside `/services/:name/*` pages: the service loaded by ServiceLayout. */
export function useServiceOutlet(): ServiceOutletContext {
  return useOutletContext<ServiceOutletContext>()
}

/** `/services/<name><sub>` with the name encoded. */
export function servicePath(name: string, sub = ''): string {
  return `/services/${encodeURIComponent(name)}${sub}`
}

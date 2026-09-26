import { useQuery } from '@tanstack/react-query'

import { ApiError, request } from '../client'
import { endpoints } from '../endpoints'
import { keys } from '../keys'

/** `GET /api/v1/info` — version, base domain, proxy URL, Docker version. */
export function useServerInfo() {
  return useQuery({
    queryKey: keys.info(),
    queryFn: ({ signal }) => endpoints.info(signal),
    staleTime: 60_000,
  })
}

/** Interactive API docs (Swagger UI) and the OpenAPI document, served by ferry-api. */
export const API_DOCS_URL = '/api/docs'
export const OPENAPI_URL = '/api/openapi.json'

export interface OpenApiSummary {
  openapi: string | null
  version: string | null
  paths: number
}

function summarizeSpec(doc: unknown): OpenApiSummary {
  const o = typeof doc === 'object' && doc !== null ? (doc as Record<string, unknown>) : {}
  const info = typeof o.info === 'object' && o.info !== null ? (o.info as Record<string, unknown>) : {}
  const paths = typeof o.paths === 'object' && o.paths !== null ? Object.keys(o.paths).length : 0
  return {
    openapi: typeof o.openapi === 'string' ? o.openapi : null,
    version: typeof info.version === 'string' ? info.version : null,
    paths,
  }
}

/**
 * `GET /api/openapi.json`, summarized. Older servers answer 404: then
 * `missing` is true and links to the API docs should be hidden.
 */
export function useOpenApiSpec({ enabled = true }: { enabled?: boolean } = {}) {
  const query = useQuery({
    queryKey: ['server', 'openapi'],
    queryFn: async ({ signal }) => summarizeSpec(await request<unknown>(OPENAPI_URL, { signal })),
    retry: false,
    staleTime: 5 * 60_000,
    enabled,
  })
  const missing = query.error instanceof ApiError && query.error.isNotFound
  return { ...query, missing, available: query.isSuccess }
}

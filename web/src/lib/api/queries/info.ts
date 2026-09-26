import { useQuery } from '@tanstack/react-query'

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

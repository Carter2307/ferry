import * as React from 'react'

/**
 * Current time, refreshed every `intervalMs`, so relative times ("3m ago")
 * stay correct while the change feed keeps queries from refetching.
 */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs)
    return () => window.clearInterval(id)
  }, [intervalMs])
  return now
}

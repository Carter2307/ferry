/** Only allow in-app redirects after login (no protocol-relative / absolute URLs, no loops). */
export function safeNext(next: string | null): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/login')) return '/services'
  return next
}

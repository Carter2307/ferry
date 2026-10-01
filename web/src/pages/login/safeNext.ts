/** Only allow in-app redirects after login (no protocol-relative / absolute URLs, no loops). */
export function safeNext(next: string | null): string {
  const loops = ['/login', '/setup'].some((p) => next?.startsWith(p))
  if (!next || !next.startsWith('/') || next.startsWith('//') || loops) return '/services'
  return next
}

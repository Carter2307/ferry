/**
 * "Firefox on macOS" from a User-Agent header: enough to tell the browsers
 * of a list of sessions apart. Unknown agents are shown as they are.
 */
export function describeUserAgent(userAgent: string | null | undefined): string {
  const ua = userAgent?.trim() ?? ''
  if (!ua) return 'Unknown browser'
  // Order matters: Edge and Opera also say Chrome, Chrome also says Safari.
  const browser = /Edg(e|A|iOS)?\//.test(ua)
    ? 'Edge'
    : /OPR\/|Opera/.test(ua)
      ? 'Opera'
      : /Firefox\/|FxiOS\//.test(ua)
        ? 'Firefox'
        : /Chrome\/|CriOS\//.test(ua)
          ? 'Chrome'
          : /Safari\//.test(ua)
            ? 'Safari'
            : /^curl\//.test(ua)
              ? 'curl'
              : null
  // iPhones and iPads also say "Mac OS X", Android also says Linux.
  const system = /iPhone|iPad|iPod/.test(ua)
    ? 'iOS'
    : /Android/.test(ua)
      ? 'Android'
      : /Windows/.test(ua)
        ? 'Windows'
        : /Mac OS X|Macintosh/.test(ua)
          ? 'macOS'
          : /Linux|X11/.test(ua)
            ? 'Linux'
            : null
  if (browser && system) return `${browser} on ${system}`
  if (browser ?? system) return (browser ?? system) as string
  return ua.length > 60 ? `${ua.slice(0, 60)}…` : ua
}

/** True on Apple platforms (⌘ vs Ctrl shortcuts). */
export const isMac: boolean =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)

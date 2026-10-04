import { useSyncExternalStore } from 'react'

const KEY = 'ferry-landing-theme'
export type Theme = 'light' | 'dark'

const systemQuery = window.matchMedia('(prefers-color-scheme: dark)')
const systemTheme = (): Theme => (systemQuery.matches ? 'dark' : 'light')

function savedTheme(): Theme | null {
  try {
    const v = localStorage.getItem(KEY)
    return v === 'light' || v === 'dark' ? v : null
  } catch {
    return null
  }
}

// One theme for the whole page: the header and the footer both have the switch, and the pixel
// fields read it for their colors. public/theme-init.js has already put the class on <html>.
let theme: Theme = savedTheme() ?? systemTheme()
const listeners = new Set<() => void>()

function setTheme(next: Theme) {
  theme = next
  document.documentElement.classList.toggle('dark', next === 'dark')
  for (const listener of listeners) listener()
}

systemQuery.addEventListener('change', () => {
  if (!savedTheme()) setTheme(systemTheme())
})

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function toggle() {
  const next = theme === 'dark' ? 'light' : 'dark'
  try {
    localStorage.setItem(KEY, next)
  } catch {
    // Not persisted (private mode, blocked storage): still applies for this visit.
  }
  setTheme(next)
}

/**
 * Light or dark, following the system until the visitor picks one (then
 * remembered in this browser). public/theme-init.js applies it before paint.
 */
export function useTheme() {
  return { theme: useSyncExternalStore(subscribe, () => theme), toggle }
}

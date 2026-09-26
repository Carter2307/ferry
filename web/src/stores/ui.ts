import { useSyncExternalStore } from 'react'
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'

export type ThemePreference = 'light' | 'dark' | 'system'
export type ResolvedTheme = 'light' | 'dark'

export interface LogPrefs {
  /** Soft-wrap long lines instead of horizontal scrolling. */
  wrap: boolean
  /** Show the timestamp column. */
  timestamps: boolean
  /** Show the instance column (runtime logs). */
  instance: boolean
}

interface UiState {
  theme: ThemePreference
  /** Icon rail shows labels when expanded (desktop only). */
  railExpanded: boolean
  /** ⌘K palette visibility (not persisted). */
  commandOpen: boolean
  /** Mobile navigation sheet (not persisted). */
  mobileNavOpen: boolean
  logPrefs: LogPrefs
  setTheme: (theme: ThemePreference) => void
  toggleRail: () => void
  setCommandOpen: (open: boolean) => void
  setMobileNavOpen: (open: boolean) => void
  setLogPrefs: (prefs: Partial<LogPrefs>) => void
}

/** In-memory stand-in when localStorage is unavailable (private mode…). */
const memory = new Map<string, string>()
const memoryStorage = {
  getItem: (k: string) => memory.get(k) ?? null,
  setItem: (k: string, v: string) => void memory.set(k, v),
  removeItem: (k: string) => void memory.delete(k),
}

/** Key read by public/theme-init.js before first paint (keep in sync). */
export const UI_KEY = 'ferry.ui'

export const useUi = create<UiState>()(
  persist(
    (set) => ({
      theme: 'system',
      railExpanded: false,
      commandOpen: false,
      mobileNavOpen: false,
      logPrefs: { wrap: true, timestamps: true, instance: true },
      setTheme: (theme) => set({ theme }),
      toggleRail: () => set((s) => ({ railExpanded: !s.railExpanded })),
      setCommandOpen: (commandOpen) => set({ commandOpen }),
      setMobileNavOpen: (mobileNavOpen) => set({ mobileNavOpen }),
      setLogPrefs: (prefs) => set((s) => ({ logPrefs: { ...s.logPrefs, ...prefs } })),
    }),
    {
      name: UI_KEY,
      version: 1,
      storage: createJSONStorage(() => {
        try {
          return window.localStorage
        } catch {
          return memoryStorage
        }
      }),
      partialize: (s) => ({ theme: s.theme, railExpanded: s.railExpanded, logPrefs: s.logPrefs }),
    },
  ),
)

const DARK_QUERY = '(prefers-color-scheme: dark)'

function subscribeSystemTheme(cb: () => void): () => void {
  const mq = window.matchMedia(DARK_QUERY)
  mq.addEventListener('change', cb)
  return () => mq.removeEventListener('change', cb)
}

function systemIsDark(): boolean {
  return window.matchMedia(DARK_QUERY).matches
}

/** The theme actually applied (system preference resolved). */
export function useResolvedTheme(): ResolvedTheme {
  const theme = useUi((s) => s.theme)
  const systemDark = useSyncExternalStore(subscribeSystemTheme, systemIsDark, () => false)
  if (theme === 'system') return systemDark ? 'dark' : 'light'
  return theme
}

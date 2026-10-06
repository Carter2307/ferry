import { Moon, Sun } from 'lucide-react'

import { useTheme } from '@/theme'

/** Switches between the light and the dark theme. The icon shows the theme the click gives. */
export function ThemeToggle({ className = '' }: { className?: string }) {
  const { theme, toggle } = useTheme()
  const dark = theme === 'dark'
  return (
    <button
      type="button"
      onClick={toggle}
      aria-label={dark ? 'Switch to the light theme' : 'Switch to the dark theme'}
      className={`btn btn-ghost btn-sm btn-icon ${className}`}
    >
      {dark ? <Sun aria-hidden="true" /> : <Moon aria-hidden="true" />}
    </button>
  )
}

import { useEffect } from 'react'

import { useUi } from '@/stores/ui'

/** Global ⌘K / Ctrl+K shortcut toggling the command palette. */
export function useCommandShortcut(): void {
  const setOpen = useUi((s) => s.setCommandOpen)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        e.preventDefault()
        setOpen(!useUi.getState().commandOpen)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setOpen])
}

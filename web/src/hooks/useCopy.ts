import * as React from 'react'
import { toast } from 'sonner'

/** Copy text to the clipboard (with a fallback for non-secure contexts). */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    /* fall through */
  }
  try {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(ta)
    return ok
  } catch {
    return false
  }
}

/** Returns [copied, copy] where `copied` flips true for 1.5s after a successful copy. */
export function useCopy(): [boolean, (text: string) => Promise<void>] {
  const [copied, setCopied] = React.useState(false)
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null)
  React.useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current)
  }, [])
  const copy = React.useCallback(async (text: string) => {
    const ok = await copyText(text)
    if (!ok) {
      toast.error('Could not copy to the clipboard')
      return
    }
    setCopied(true)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => setCopied(false), 1500)
  }, [])
  return [copied, copy]
}

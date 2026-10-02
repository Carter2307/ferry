/**
 * Blueprint editor draft, kept in sessionStorage (this tab only) so a reload
 * or a detour to another page doesn't lose the YAML. The draft is tied to the
 * signed-in account and cleared on sign-out.
 */

import { useAuth } from '@/stores/auth'

const KEY = 'ferry.blueprints.draft'

export interface BlueprintDraft {
  yaml: string
  fileName: string | null
}

interface StoredDraft extends BlueprintDraft {
  v: 1
  owner: string
}

/** Whose draft it is: the id of the signed-in account. */
function owner(): string {
  return useAuth.getState().user?.id ?? ''
}

function storage(): Storage | null {
  try {
    return window.sessionStorage
  } catch {
    return null
  }
}

function isStoredDraft(v: unknown): v is StoredDraft {
  if (typeof v !== 'object' || v === null) return false
  const o = v as Record<string, unknown>
  return (
    o.v === 1 &&
    typeof o.owner === 'string' &&
    typeof o.yaml === 'string' &&
    (o.fileName === null || typeof o.fileName === 'string')
  )
}

export function loadDraft(): BlueprintDraft | null {
  try {
    const raw = storage()?.getItem(KEY)
    if (!raw) return null
    const parsed: unknown = JSON.parse(raw)
    if (!isStoredDraft(parsed) || parsed.owner !== owner()) return null
    return { yaml: parsed.yaml, fileName: parsed.fileName }
  } catch {
    return null
  }
}

export function saveDraft(draft: BlueprintDraft): void {
  try {
    if (draft.yaml === '' && draft.fileName === null) {
      storage()?.removeItem(KEY)
      return
    }
    const stored: StoredDraft = { v: 1, owner: owner(), ...draft }
    storage()?.setItem(KEY, JSON.stringify(stored))
  } catch {
    /* quota / disabled storage: the draft just isn't kept */
  }
}

export function clearDraft(): void {
  try {
    storage()?.removeItem(KEY)
  } catch {
    /* storage unavailable */
  }
}

// Sign-out clears the draft (the subscription lives as long as this module,
// i.e. from the first visit of the page; the owner check covers the rest).
useAuth.subscribe((state, prev) => {
  if (prev.phase === 'signed-in' && state.phase !== 'signed-in') clearDraft()
})

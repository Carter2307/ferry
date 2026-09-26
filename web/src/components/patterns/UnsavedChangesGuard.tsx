import { useUnsavedChangesGuard } from './useUnsavedChangesGuard'

/** Component form of {@link useUnsavedChangesGuard} for pages that never bypass it. */
export function UnsavedChangesGuard({ when, description }: { when: boolean; description?: string }) {
  return useUnsavedChangesGuard(when, { description }).dialog
}

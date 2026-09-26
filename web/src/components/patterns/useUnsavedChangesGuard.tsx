import * as React from 'react'
import { useBlocker } from 'react-router'

import { ConfirmDialog } from './ConfirmDialog'

const DEFAULT_DESCRIPTION = 'You have edits on this page that are not saved yet. If you leave now, they will be lost.'

/**
 * Warns before leaving a page with unsaved edits: an in-app confirmation for
 * router navigations (other page, other resource, back button) and the
 * browser's own prompt for reloads / closing the tab.
 *
 * Returns the dialog to render and `bypass()` for intentional navigations
 * (e.g. right after deleting the resource). React Router supports one blocker
 * at a time, so use it once per page with the OR of every form's dirty flag.
 */
export function useUnsavedChangesGuard(dirty: boolean, { description = DEFAULT_DESCRIPTION }: { description?: string } = {}) {
  const bypassRef = React.useRef(false)
  const proceedingRef = React.useRef(false)
  const blocker = useBlocker(
    React.useCallback(
      ({ currentLocation, nextLocation }: { currentLocation: { pathname: string }; nextLocation: { pathname: string } }) =>
        dirty && !bypassRef.current && currentLocation.pathname !== nextLocation.pathname,
      [dirty],
    ),
  )

  React.useEffect(() => {
    if (!dirty) return
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault()
      // Older browsers need returnValue set to show the prompt.
      e.returnValue = ''
    }
    window.addEventListener('beforeunload', onBeforeUnload)
    return () => window.removeEventListener('beforeunload', onBeforeUnload)
  }, [dirty])

  // The edits were saved or discarded while the prompt was up: let the navigation through.
  React.useEffect(() => {
    if (blocker.state === 'blocked' && !dirty) blocker.proceed()
  }, [blocker, dirty])

  const dialog = (
    <ConfirmDialog
      open={blocker.state === 'blocked'}
      onOpenChange={(open) => {
        if (open || proceedingRef.current) return
        if (blocker.state === 'blocked') blocker.reset()
      }}
      title="Discard unsaved changes?"
      description={description}
      confirmLabel="Discard changes"
      cancelLabel="Keep editing"
      variant="danger"
      onConfirm={() => {
        if (blocker.state !== 'blocked') return
        proceedingRef.current = true
        blocker.proceed()
        // The page usually unmounts on navigation; reset in case it doesn't.
        setTimeout(() => {
          proceedingRef.current = false
        }, 0)
      }}
    />
  )

  const bypass = React.useCallback(() => {
    bypassRef.current = true
  }, [])

  return { dialog, bypass }
}

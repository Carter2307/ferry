import { FerryLogo } from '@/components/patterns/icons'

/** Shown while the first route chunk loads. */
export function FullPageLoader() {
  return (
    <div className="flex h-dvh items-center justify-center bg-background" role="status" aria-label="Loading">
      <FerryLogo className="size-8 animate-pulse" />
    </div>
  )
}

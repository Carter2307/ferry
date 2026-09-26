import type * as React from 'react'
import { CircleCheck, Info, Loader2, OctagonX, TriangleAlert } from 'lucide-react'
import { Toaster as Sonner, type ToasterProps } from 'sonner'

import { useResolvedTheme } from '@/stores/ui'

/** Supabase-style toasts: raised surface, hairline border, 8px radius, colored icon. */
function Toaster(props: ToasterProps) {
  const theme = useResolvedTheme()
  return (
    <Sonner
      theme={theme}
      position="bottom-right"
      className="toaster group"
      closeButton
      icons={{
        success: <CircleCheck className="size-4 text-primary" />,
        info: <Info className="size-4 text-info" />,
        warning: <TriangleAlert className="size-4 text-warning" />,
        error: <OctagonX className="size-4 text-destructive" />,
        loading: <Loader2 className="size-4 animate-spin text-foreground-lighter" />,
      }}
      toastOptions={{
        classNames: {
          toast: '!items-start !gap-2.5 !font-sans !text-[13px] !shadow-overlay',
          title: '!font-medium !text-foreground',
          description: '!text-foreground-light !text-[13px]',
          icon: '!mt-px',
          closeButton: '!border-border-strong !bg-popover !text-foreground-lighter hover:!text-foreground',
          actionButton: '!bg-primary-solid !text-primary-foreground !rounded-md !font-medium',
          cancelButton: '!bg-surface-200 !text-foreground !rounded-md',
        },
      }}
      style={
        {
          '--normal-bg': 'var(--popover)',
          '--normal-text': 'var(--popover-foreground)',
          '--normal-border': 'var(--border-strong)',
          '--border-radius': '8px',
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { Toaster }

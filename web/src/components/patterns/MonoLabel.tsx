import * as React from 'react'

import { cn } from '@/lib/utils'

/** The Supabase signature: small UPPERCASE monospace label (card titles, table headers, menu groups). */
export function MonoLabel({
  className,
  as: Comp = 'span',
  ...props
}: React.HTMLAttributes<HTMLElement> & { as?: 'span' | 'div' | 'h2' | 'h3' | 'h4' | 'p' | 'dt' | 'label' }) {
  return <Comp className={cn('mono-label', className)} {...props} />
}

import * as React from 'react'

import { Skeleton } from '@/components/ui/skeleton'
import { TableCell, TableRow } from '@/components/ui/table'
import { errorMessage } from '@/lib/api/client'
import { cn } from '@/lib/utils'

/** `rows` placeholder rows while a table loads. */
export function TableSkeletonRows({ columns, rows = 4 }: { columns: number; rows?: number }) {
  return (
    <>
      {Array.from({ length: rows }, (_, r) => (
        <TableRow key={r} className="hover:bg-transparent">
          {Array.from({ length: columns }, (_, c) => (
            <TableCell key={c}>
              <Skeleton className={cn('h-4', c === 0 ? 'w-32' : c === columns - 1 ? 'w-12' : 'w-20')} />
            </TableCell>
          ))}
        </TableRow>
      ))}
    </>
  )
}

/** Single full-width row for empty / error states inside a table body. */
export function TableMessageRow({
  colSpan,
  children,
  tone = 'muted',
}: {
  colSpan: number
  children: React.ReactNode
  tone?: 'muted' | 'error'
}) {
  return (
    <TableRow className="hover:bg-transparent">
      <TableCell colSpan={colSpan} className={cn('h-24 text-center whitespace-normal', tone === 'error' ? 'text-destructive' : 'text-foreground-light')}>
        {children}
      </TableCell>
    </TableRow>
  )
}

/** Error row showing the API message. */
export function TableErrorRow({ colSpan, error }: { colSpan: number; error: unknown }) {
  return (
    <TableMessageRow colSpan={colSpan} tone="error">
      {errorMessage(error)}
    </TableMessageRow>
  )
}

import type * as React from 'react'

/**
 * Makes a whole row keyboard/click navigable (use with an inner <Link> for
 * the accessible name): `<TableRow {...rowLinkProps(() => navigate(url))}>`.
 */
export function rowLinkProps(onOpen: () => void): React.HTMLAttributes<HTMLTableRowElement> {
  return {
    className: 'cursor-pointer',
    onClick: (e) => {
      if ((e.target as HTMLElement).closest('a,button,input,[role=menuitem]')) return
      onOpen()
    },
  }
}

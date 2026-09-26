import { Link2 } from 'lucide-react'

import { DatastoreKindIcon } from '@/components/patterns/icons'
import { mergeRows, type KvRow } from '@/components/patterns/kv-rows'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useDatastores } from '@/lib/api/queries'
import type { DatastoreView } from '@/lib/api/types'
import { DATASTORE_KIND_LABELS } from '@/lib/format'

/** A key not used by `rows` for a reference to `ds` (DATABASE_URL, then APP_DB_URL…). */
function keyFor(ds: DatastoreView, rows: KvRow[]): string {
  const used = new Set(rows.map((r) => r.key.trim()))
  const preferred = ds.kind === 'postgres' ? 'DATABASE_URL' : 'REDIS_URL'
  if (!used.has(preferred)) return preferred
  const base = `${ds.name.toUpperCase().replace(/[^A-Z0-9]+/g, '_')}_URL`
  let key = base
  for (let i = 2; used.has(key); i++) key = `${base}_${i}`
  return key
}

/**
 * "Reference a datastore" menu: adds `DATABASE_URL=${{datastore.<name>.connectionString}}`
 * (resolved by the server at deploy time, so credentials never live in the service).
 */
export function DatastoreRefMenu({ rows, onChange }: { rows: KvRow[]; onChange: (rows: KvRow[]) => void }) {
  const datastores = useDatastores()
  const list = datastores.data ?? []
  if (list.length === 0) return null
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="tiny" variant="ghost" icon={<Link2 />}>
          Reference a datastore
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel>Add connection string</DropdownMenuLabel>
        {list.map((ds) => (
          <DropdownMenuItem
            key={ds.id}
            onSelect={() =>
              onChange(mergeRows(rows, [{ key: keyFor(ds, rows), value: `\${{datastore.${ds.name}.connectionString}}` }]))
            }
          >
            <DatastoreKindIcon kind={ds.kind} />
            <span className="flex-1 truncate font-mono text-[12.5px] text-foreground">{ds.name}</span>
            <span className="text-[12px] text-foreground-lighter">{DATASTORE_KIND_LABELS[ds.kind]}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

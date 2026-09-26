import * as React from 'react'
import { Link } from 'react-router'
import { Braces, Check, ChevronsUpDown, X } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/api/client'
import { useEnvGroups } from '@/lib/api/queries'
import { cn } from '@/lib/utils'

interface EnvGroupsPickerProps {
  id: string
  value: string[]
  onChange: (groups: string[]) => void
  invalid?: boolean
  describedBy?: string
}

/** Multi-select of env groups (searchable popover + removable chips). */
export function EnvGroupsPicker({ id, value, onChange, invalid, describedBy }: EnvGroupsPickerProps) {
  const groups = useEnvGroups()
  const [open, setOpen] = React.useState(false)

  if (groups.isPending) return <Skeleton className="h-[34px] w-full" />
  if (groups.isError) {
    return (
      <p role="alert" className="text-[13px] text-destructive">
        Could not load env groups: {errorMessage(groups.error)}
      </p>
    )
  }
  const all = groups.data
  if (all.length === 0) {
    return (
      <p className="text-[13px] text-foreground-light">
        No env groups yet.{' '}
        <Link to="/env-groups" className="text-primary underline-offset-4 hover:underline">
          Create one
        </Link>{' '}
        to share variables between services.
      </p>
    )
  }

  const toggle = (name: string) =>
    onChange(value.includes(name) ? value.filter((g) => g !== name) : [...value, name])

  return (
    <div className="flex flex-col gap-2">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            id={id}
            size="md"
            role="combobox"
            aria-expanded={open}
            aria-invalid={invalid || undefined}
            aria-describedby={describedBy}
            iconRight={<ChevronsUpDown className="ml-auto size-3.5 text-foreground-lighter" />}
            className="w-full justify-start font-normal dark:bg-surface-200"
          >
            <Braces className="size-3.5 text-foreground-lighter" />
            <span className={cn('truncate', value.length === 0 && 'text-foreground-muted')}>
              {value.length === 0 ? 'Link env groups' : `${value.length} group${value.length === 1 ? '' : 's'} linked`}
            </span>
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-64 p-0">
          <Command>
            <CommandInput placeholder="Search env groups…" />
            <CommandList>
              <CommandEmpty>No env group found.</CommandEmpty>
              <CommandGroup>
                {all.map((g) => {
                  const selected = value.includes(g.name)
                  return (
                    <CommandItem key={g.id} value={g.name} onSelect={() => toggle(g.name)}>
                      <span
                        aria-hidden="true"
                        className={cn(
                          'flex size-4 items-center justify-center rounded-[4px] border',
                          selected ? 'border-primary-solid-border bg-primary-solid text-primary-foreground' : 'border-border-stronger',
                        )}
                      >
                        {selected && <Check className="size-3 text-primary-foreground" strokeWidth={3} />}
                      </span>
                      <span className="flex-1 truncate font-mono text-[12.5px] text-foreground">
                        {g.name}
                        {selected && <span className="sr-only"> (linked)</span>}
                      </span>
                      <span className="text-[12px] text-foreground-lighter">
                        {g.vars.length} var{g.vars.length === 1 ? '' : 's'}
                      </span>
                    </CommandItem>
                  )
                })}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {value.length > 0 && (
        <ul className="flex flex-wrap gap-1.5" aria-label="Linked env groups">
          {value.map((name) => (
            <li
              key={name}
              className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border-strong bg-surface-200 pr-1 pl-2 font-mono text-[12.5px] text-foreground"
            >
              <Braces className="size-3.5 text-foreground-lighter" aria-hidden="true" />
              {name}
              <button
                type="button"
                onClick={() => toggle(name)}
                className="flex size-5 items-center justify-center rounded-sm text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                aria-label={`Unlink ${name}`}
              >
                <X className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

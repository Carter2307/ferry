import * as React from 'react'
import { Check, ChevronsUpDown, GitBranch, Lock } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/api/client'
import { useGitBranches } from '@/lib/api/queries'
import { cn } from '@/lib/utils'

import { validateBranch } from './form'

interface BranchSelectProps {
  id: string
  /** The connection and repository whose branches are listed. */
  connectionId: string
  repository: string
  /** The repository's default branch, listed first. */
  defaultBranch: string | null
  value: string
  onChange: (branch: string) => void
  onBlur?: () => void
  invalid?: boolean
  describedBy?: string
}

/**
 * Branch of a repository picked from a connected account: a searchable list
 * of its branches (asked from the provider when opened), which also takes a
 * name typed by hand (a branch that isn't listed, or not pushed yet).
 */
export function BranchSelect({
  id,
  connectionId,
  repository,
  defaultBranch,
  value,
  onChange,
  onBlur,
  invalid,
  describedBy,
}: BranchSelectProps) {
  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const branches = useGitBranches(connectionId, repository, { enabled: open })

  const listed = React.useMemo(() => {
    const all = branches.data ?? []
    // The default branch first, then the provider's order.
    return [...all].sort((a, b) => Number(b.name === defaultBranch) - Number(a.name === defaultBranch))
  }, [branches.data, defaultBranch])

  const typed = search.trim()
  const canUseTyped = typed !== '' && validateBranch(typed) === null && !listed.some((b) => b.name === typed)
  const choose = (branch: string) => {
    onChange(branch)
    setOpen(false)
  }

  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        setOpen(o)
        if (o) setSearch('')
        else onBlur?.()
      }}
    >
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
          <GitBranch className="size-3.5 text-foreground-lighter" />
          <span className={cn('truncate font-mono text-[13px]', !value && 'text-foreground-muted')}>
            {value || 'Select a branch'}
          </span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-64 p-0">
        <Command>
          <CommandInput placeholder="Search or type a branch…" value={search} onValueChange={setSearch} />
          <CommandList className="max-h-[min(280px,50dvh)]">
            {branches.isPending ? (
              <div className="flex flex-col gap-1.5 p-2" aria-label="Loading branches">
                {[0, 1, 2].map((i) => (
                  <Skeleton key={i} className="h-7 w-full" />
                ))}
              </div>
            ) : (
              <>
                {branches.isError && (
                  <p role="alert" className="px-3 pt-2.5 pb-1 text-[12.5px] text-destructive">
                    Could not list the branches: {errorMessage(branches.error)}
                  </p>
                )}
                <CommandEmpty>
                  {typed === ''
                    ? 'Type a branch name.'
                    : validateBranch(typed)
                      ? 'This is not a valid branch name.'
                      : 'No branch found.'}
                </CommandEmpty>
                {listed.length > 0 && (
                  <CommandGroup>
                    {listed.map((b) => (
                      <CommandItem key={b.name} value={b.name} onSelect={() => choose(b.name)}>
                        <Check className={cn('size-3.5 text-primary', b.name !== value && 'invisible')} />
                        <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-foreground">{b.name}</span>
                        {b.name === defaultBranch && <span className="text-[12px] text-foreground-lighter">default</span>}
                        {b.protected && (
                          <>
                            <Lock className="size-3 text-foreground-lighter" aria-hidden="true" />
                            <span className="sr-only">protected</span>
                          </>
                        )}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                )}
                {canUseTyped && (
                  <CommandGroup>
                    <CommandItem value={typed} onSelect={() => choose(typed)}>
                      <GitBranch className="size-3.5" />
                      <span className="min-w-0 flex-1 truncate">
                        Use <span className="font-mono text-[12.5px] text-foreground">{typed}</span>
                      </span>
                    </CommandItem>
                  </CommandGroup>
                )}
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

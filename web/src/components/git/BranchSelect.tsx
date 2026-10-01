import * as React from 'react'
import { Check, ChevronsUpDown, GitBranch } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/api/client'
import { useGitBranches } from '@/lib/api/queries'
import { isBranchName } from '@/lib/git'
import { cn } from '@/lib/utils'

interface BranchSelectProps {
  id: string
  /**
   * The repository whose branches are listed: any URL a service can deploy
   * from. Empty (or not a repository URL yet): nothing to list, a name can
   * still be typed.
   */
  repoUrl: string
  value: string
  onChange: (branch: string) => void
  onBlur?: () => void
  invalid?: boolean
  describedBy?: string
  className?: string
}

/**
 * The branch of a repository: a searchable list of its branches, asked from
 * the repository itself when the list opens (the server reads it the way a
 * deploy would clone it, with the connected account that serves it). A name
 * can also be typed by hand: a branch that isn't pushed yet, or a repository
 * the server can't read from here.
 */
export function BranchSelect({
  id,
  repoUrl,
  value,
  onChange,
  onBlur,
  invalid,
  describedBy,
  className,
}: BranchSelectProps) {
  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const repository = repoUrl.trim()
  const branches = useGitBranches(repository, { enabled: open && repository !== '' })
  const listed = branches.data?.branches ?? []
  const defaultBranch = branches.data?.default_branch ?? null

  const typed = search.trim()
  const canUseTyped = typed !== '' && isBranchName(typed) && !listed.includes(typed)
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
          className={cn('w-full justify-start font-normal dark:bg-surface-200', className)}
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
            {repository !== '' && branches.isPending ? (
              <div className="flex flex-col gap-1.5 p-2" aria-label="Loading branches" aria-busy="true">
                {[0, 1, 2].map((i) => (
                  <Skeleton key={i} className="h-7 w-full" />
                ))}
              </div>
            ) : (
              <>
                {branches.isError && (
                  <p role="alert" className="px-3 pt-2.5 pb-1 text-[12.5px] leading-relaxed break-words text-destructive">
                    {errorMessage(branches.error)}
                  </p>
                )}
                <CommandEmpty>
                  {typed !== '' && !isBranchName(typed)
                    ? 'This is not a valid branch name.'
                    : repository === ''
                      ? 'Choose the repository to list its branches, or type a branch name.'
                      : typed === ''
                        ? 'Type a branch name.'
                        : 'No branch found.'}
                </CommandEmpty>
                {listed.length > 0 && (
                  <CommandGroup>
                    {listed.map((name) => (
                      <CommandItem key={name} value={name} onSelect={() => choose(name)}>
                        <Check className={cn('size-3.5 text-primary', name !== value && 'invisible')} />
                        <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-foreground">{name}</span>
                        {name === defaultBranch && <span className="text-[12px] text-foreground-lighter">default</span>}
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

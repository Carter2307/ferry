import * as React from 'react'
import { ChevronDown, Search, X } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'

/**
 * Toolbar search field of the resource lists (`🔍 Search for a project`):
 * leading icon, clear button, Escape clears.
 */
export function SearchInput({
  value,
  onChange,
  placeholder,
  label,
  className,
}: {
  value: string
  onChange: (value: string) => void
  placeholder: string
  /** Accessible name (defaults to the placeholder). */
  label?: string
  className?: string
}) {
  const ref = React.useRef<HTMLInputElement | null>(null)
  return (
    <div className={cn('relative w-full sm:w-auto sm:max-w-[320px] sm:min-w-[200px] sm:flex-1', className)}>
      <Search
        className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-foreground-lighter"
        aria-hidden="true"
      />
      <Input
        ref={ref}
        type="search"
        size="sm"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && value) {
            e.preventDefault()
            onChange('')
          }
        }}
        placeholder={placeholder}
        aria-label={label ?? placeholder}
        autoComplete="off"
        spellCheck={false}
        className="pr-8 pl-8 [&::-webkit-search-cancel-button]:hidden"
      />
      {value && (
        <button
          type="button"
          onClick={() => {
            onChange('')
            ref.current?.focus()
          }}
          className="absolute top-1/2 right-1.5 flex size-5 -translate-y-1/2 items-center justify-center rounded-sm text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="Clear search"
        >
          <X className="size-3.5" />
        </button>
      )}
    </div>
  )
}

/**
 * Studio dashed filter button (`Status ⌄`): label, count of selected values,
 * trailing chevron; turns solid while a filter is active. Use it as the
 * `asChild` child of a Popover / DropdownMenu trigger.
 */
export function FilterButton({
  label,
  selected = [],
  className,
  ...props
}: Omit<React.ComponentProps<typeof Button>, 'children' | 'variant'> & {
  label: string
  /** Labels of the selected values (count badge + accessible summary). */
  selected?: string[]
}) {
  const summary = selected.join(', ')
  return (
    <Button
      variant="dashed"
      iconRight={<ChevronDown className="size-3.5 opacity-70" />}
      aria-label={summary ? `${label} filter: ${summary}` : `Filter by ${label.toLowerCase()}`}
      title={summary || undefined}
      className={cn(selected.length > 0 && 'border-solid border-border-strong text-foreground', className)}
      {...props}
    >
      {label}
      {selected.length > 0 && (
        <span
          aria-hidden="true"
          className="inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-primary-soft px-1 font-mono text-[10.5px] text-primary tabular"
        >
          {selected.length}
        </span>
      )}
    </Button>
  )
}

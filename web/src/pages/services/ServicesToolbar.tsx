import { Link } from 'react-router'
import { ArrowUpDown, LayoutGrid, List, Plus } from 'lucide-react'

import { FilterButton, SearchInput } from '@/components/patterns/ListToolbar'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { SERVICE_STATE_TONE } from '@/components/patterns/status-tones'
import { StatusDot } from '@/components/patterns/StatusBadge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Hint } from '@/components/ui/tooltip'
import type { ServiceState } from '@/lib/api/types'
import { SERVICE_STATE_LABELS } from '@/lib/format'

import { FILTER_STATES, type ServicesSort, type ServicesView } from './lib'

const SORT_LABELS: Record<ServicesSort, string> = { name: 'name', last_deploy: 'last deploy' }

function StatusFilter({
  selected,
  counts,
  onChange,
}: {
  selected: ServiceState[]
  counts: Record<ServiceState, number>
  onChange: (states: ServiceState[]) => void
}) {
  const toggle = (state: ServiceState, on: boolean) =>
    onChange(FILTER_STATES.filter((s) => (s === state ? on : selected.includes(s))))

  return (
    <Popover>
      <PopoverTrigger asChild>
        <FilterButton label="Status" selected={selected.map((s) => SERVICE_STATE_LABELS[s])} />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-60 p-0">
        <fieldset className="flex flex-col py-1.5">
          <legend className="sr-only">Service status</legend>
          <MonoLabel as="div" className="px-3 pt-1.5 pb-1 text-[11px]">
            Filter by status
          </MonoLabel>
          {FILTER_STATES.map((state) => {
            const id = `status-filter-${state}`
            return (
              <label
                key={state}
                htmlFor={id}
                className="mx-1.5 flex cursor-pointer items-center gap-2.5 rounded-[5px] px-1.5 py-1.5 text-[13px] text-foreground-light hover:bg-surface-200 hover:text-foreground"
              >
                <Checkbox
                  id={id}
                  aria-label={SERVICE_STATE_LABELS[state]}
                  checked={selected.includes(state)}
                  onCheckedChange={(c) => toggle(state, c === true)}
                />
                <StatusDot tone={SERVICE_STATE_TONE[state].tone} />
                <span className="flex-1">{SERVICE_STATE_LABELS[state]}</span>
                <span className="font-mono text-[11.5px] text-foreground-lighter tabular">{counts[state]}</span>
              </label>
            )
          })}
        </fieldset>
        {selected.length > 0 && (
          <div className="border-t p-1.5">
            <Button variant="ghost" size="tiny" className="w-full justify-center" onClick={() => onChange([])}>
              Clear filter
            </Button>
          </div>
        )}
      </PopoverContent>
    </Popover>
  )
}

function SortMenu({ sort, onChange }: { sort: ServicesSort; onChange: (s: ServicesSort) => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button icon={<ArrowUpDown className="size-3.5" />} aria-label={`Sorted by ${SORT_LABELS[sort]}. Change sort order`}>
          <span className="hidden xl:inline">Sorted by {SORT_LABELS[sort]}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-48">
        <DropdownMenuLabel>Sort by</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={sort} onValueChange={(v) => onChange(v === 'last_deploy' ? 'last_deploy' : 'name')}>
          <DropdownMenuRadioItem value="name">Name</DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="last_deploy">Last deploy</DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

interface ServicesToolbarProps {
  query: string
  onQueryChange: (q: string) => void
  states: ServiceState[]
  counts: Record<ServiceState, number>
  onStatesChange: (states: ServiceState[]) => void
  sort: ServicesSort
  onSortChange: (s: ServicesSort) => void
  view: ServicesView
  onViewChange: (v: ServicesView) => void
}

/** Studio "Projects" toolbar: search, dashed Status filter, sort, grid/list toggle, + New service. */
export function ServicesToolbar(props: ServicesToolbarProps) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <SearchInput
        value={props.query}
        onChange={props.onQueryChange}
        placeholder="Search for a service"
        label="Search services"
      />
      <StatusFilter selected={props.states} counts={props.counts} onChange={props.onStatesChange} />
      <SortMenu sort={props.sort} onChange={props.onSortChange} />
      <div className="ml-auto flex items-center gap-2">
        <ToggleGroup
          type="single"
          variant="outline"
          value={props.view}
          onValueChange={(v) => {
            if (v === 'grid' || v === 'list') props.onViewChange(v)
          }}
          aria-label="Layout"
        >
          <Hint label="Grid view">
            <ToggleGroupItem value="grid" aria-label="Grid view" className="px-2">
              <LayoutGrid />
            </ToggleGroupItem>
          </Hint>
          <Hint label="List view">
            <ToggleGroupItem value="list" aria-label="List view" className="px-2">
              <List />
            </ToggleGroupItem>
          </Hint>
        </ToggleGroup>
        <Button asChild variant="primary" icon={<Plus />}>
          <Link to="/services/new">New service</Link>
        </Button>
      </div>
    </div>
  )
}

import * as React from 'react'

import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import type { ServerInfo } from '@/lib/api/types'
import {
  chooseLimit,
  formatCpus,
  formatMemoryMb,
  LIMIT_CUSTOM,
  limitOptions,
  readLimit,
  type LimitField,
  type LimitKind,
} from '@/lib/resources'
import { cn } from '@/lib/utils'

const PLACEHOLDER: Record<LimitKind, string> = { memory: 'e.g. 768M or 1.5G', cpu: 'e.g. 0.75 or 1500m' }
const CUSTOM_LABEL: Record<LimitKind, string> = { memory: 'Custom memory limit', cpu: 'Custom CPU limit' }

interface LimitControlProps {
  kind: LimitKind
  /** id of the select trigger (label `for`); the custom input gets `${id}-custom`. */
  id: string
  value: LimitField
  onChange: (value: LimitField) => void
  /** `/api/v1/info`: labels the server default, flags presets above the Docker host. */
  info: ServerInfo | null | undefined
  invalid?: boolean
  describedBy?: string
  disabled?: boolean
  className?: string
}

/**
 * Memory / CPU limit picker: a select (Server default (512 MiB) · presets ·
 * Custom…) and, for Custom…, a size input (`768M`, `1.5G` / `0.75`, `1500m`)
 * with the parsed value next to it.
 */
export function LimitControl({
  kind,
  id,
  value,
  onChange,
  info,
  invalid,
  describedBy,
  disabled,
  className,
}: LimitControlProps) {
  const options = limitOptions(kind, info)
  const custom = value.choice === LIMIT_CUSTOM
  const inputRef = React.useRef<HTMLInputElement>(null)
  const focusCustom = React.useRef(false)
  const parsed = custom ? readLimit(kind, value) : null
  const formatted =
    parsed?.ok && parsed.value !== null ? (kind === 'memory' ? formatMemoryMb(parsed.value) : formatCpus(parsed.value)) : null
  // `1.5G` → `= 1.5 GiB`; nothing when the text already reads that way.
  const preview =
    formatted && !invalid && formatted.toLowerCase() !== value.custom.trim().toLowerCase() ? formatted : null

  return (
    <div className={cn('flex min-w-0 flex-col gap-2', className)}>
      <Select
        value={value.choice}
        disabled={disabled}
        onValueChange={(choice) => {
          focusCustom.current = choice === LIMIT_CUSTOM
          onChange(chooseLimit(kind, value, choice))
        }}
      >
        <SelectTrigger
          id={id}
          className="w-full"
          aria-invalid={invalid && !custom ? true : undefined}
          aria-describedby={describedBy}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent
          position="popper"
          onCloseAutoFocus={(e) => {
            // Picking Custom… moves focus to the size input instead of back to the select.
            if (!focusCustom.current) return
            focusCustom.current = false
            e.preventDefault()
            requestAnimationFrame(() => inputRef.current?.focus())
          }}
        >
          {options.map((o) => (
            <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
              {o.label}
              {o.note && <span className="text-foreground-lighter">· {o.note}</span>}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {custom && (
        <div className="relative">
          <Input
            ref={inputRef}
            id={`${id}-custom`}
            mono
            autoComplete="off"
            spellCheck={false}
            inputMode="decimal"
            aria-label={CUSTOM_LABEL[kind]}
            placeholder={PLACEHOLDER[kind]}
            value={value.custom}
            disabled={disabled}
            onChange={(e) => onChange({ ...value, custom: e.target.value })}
            aria-invalid={invalid ? true : undefined}
            aria-describedby={describedBy}
            className={preview ? 'pr-24' : undefined}
          />
          {preview && (
            <span
              aria-hidden="true"
              className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-[12.5px] text-foreground-lighter tabular"
            >
              = {preview}
            </span>
          )}
        </div>
      )}
    </div>
  )
}

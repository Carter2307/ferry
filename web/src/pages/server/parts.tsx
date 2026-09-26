import * as React from 'react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'

import { CopyButton } from '@/components/patterns/Copy'
import { StatePill } from '@/components/patterns/StatusBadge'
import { cn } from '@/lib/utils'
import { useUi, type ThemePreference } from '@/stores/ui'

/**
 * Command snippet whose visible text hides secrets (`display`) while the copy
 * button copies the real command (`value`).
 */
export function MaskedCommand({
  display,
  value,
  what,
  prompt = true,
}: {
  display: string
  value: string
  what: string
  prompt?: boolean
}) {
  return (
    <div className="relative w-full rounded-md border bg-surface-200 text-left">
      <pre className="overflow-x-auto px-3 py-2.5 pr-12 font-mono text-[12.5px] leading-relaxed text-foreground">
        {display.split('\n').map((line, i) => (
          <React.Fragment key={i}>
            {i > 0 && '\n'}
            {prompt && <span className="text-foreground-muted select-none">$ </span>}
            {line}
          </React.Fragment>
        ))}
      </pre>
      <CopyButton value={value} what={what} className="absolute top-1.5 right-1.5" />
    </div>
  )
}

/** `● ENABLED` / `○ DISABLED` pill. */
export function OnOffPill({
  on,
  onLabel = 'Enabled',
  offLabel = 'Disabled',
}: {
  on: boolean
  onLabel?: string
  offLabel?: string
}) {
  return <StatePill tone={on ? 'success' : 'neutral'} label={on ? onLabel : offLabel} />
}

/** Value cell aligned with 34px controls (text values in form rows). */
export function RowValue({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div className={cn('flex min-h-[34px] flex-wrap items-center gap-2 text-sm text-foreground', className)}>
      {children}
    </div>
  )
}

const PREVIEW = {
  light: { bg: '#fcfcfc', panel: '#ffffff', line: '#e4e4e4', text: '#d4d4d4', rail: '#f3f3f3' },
  dark: { bg: '#1c1c1c', panel: '#232323', line: '#343434', text: '#454545', rail: '#1f1f1f' },
} as const

function MiniWindow({ mode }: { mode: 'light' | 'dark' }) {
  const c = PREVIEW[mode]
  return (
    <g>
      <rect width="120" height="76" fill={c.bg} />
      <rect width="18" height="76" fill={c.rail} />
      <rect x="18" width="1" height="76" fill={c.line} />
      <rect x="0" y="12" width="120" height="1" fill={c.line} />
      <circle cx="9" cy="6" r="2.5" fill="#3ecf8e" />
      <rect x="28" y="22" width="36" height="5" rx="2" fill={c.text} />
      <rect x="28" y="33" width="82" height="34" rx="3" fill={c.panel} stroke={c.line} />
      <rect x="34" y="40" width="30" height="3" rx="1.5" fill={c.text} />
      <rect x="34" y="47" width="46" height="3" rx="1.5" fill={c.text} />
      <rect x="88" y="56" width="16" height="6" rx="2" fill="#3ecf8e" />
    </g>
  )
}

function ThemePreview({ theme }: { theme: ThemePreference }) {
  const clipId = React.useId()
  return (
    <svg viewBox="0 0 120 76" aria-hidden="true" className="block h-auto w-full">
      {theme === 'system' ? (
        <>
          <defs>
            <clipPath id={clipId}>
              <polygon points="120,0 120,76 0,76" />
            </clipPath>
          </defs>
          <MiniWindow mode="light" />
          <g clipPath={`url(#${clipId})`}>
            <MiniWindow mode="dark" />
          </g>
        </>
      ) : (
        <MiniWindow mode={theme} />
      )}
    </svg>
  )
}

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
]

/** Supabase-style appearance picker: three preview cards. */
export function ThemePicker({ 'aria-labelledby': labelledBy }: { 'aria-labelledby'?: string }) {
  const theme = useUi((s) => s.theme)
  const setTheme = useUi((s) => s.setTheme)
  return (
    <RadioGroupPrimitive.Root
      aria-labelledby={labelledBy}
      value={theme}
      onValueChange={(v) => {
        const next = THEMES.find((t) => t.value === v)
        if (next) setTheme(next.value)
      }}
      className="grid grid-cols-3 gap-3"
    >
      {THEMES.map((t) => (
        <RadioGroupPrimitive.Item
          key={t.value}
          value={t.value}
          className="group flex cursor-pointer flex-col gap-2 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <span className="overflow-hidden rounded-md border border-border-strong transition-[border-color,box-shadow] group-hover:border-border-stronger group-data-[state=checked]:border-primary-bright group-data-[state=checked]:ring-2 group-data-[state=checked]:ring-ring">
            <ThemePreview theme={t.value} />
          </span>
          <span className="flex items-center gap-2 text-[13px] text-foreground-light group-data-[state=checked]:font-medium group-data-[state=checked]:text-foreground">
            <span
              aria-hidden="true"
              className="flex size-3.5 items-center justify-center rounded-full border border-border-stronger group-data-[state=checked]:border-primary-solid-border group-data-[state=checked]:bg-primary-solid"
            >
              <span className="size-1.5 rounded-full bg-white opacity-0 group-data-[state=checked]:opacity-100" />
            </span>
            {t.label}
          </span>
        </RadioGroupPrimitive.Item>
      ))}
    </RadioGroupPrimitive.Root>
  )
}

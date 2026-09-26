import * as React from 'react'
import { Box, GitBranch, TriangleAlert, Upload } from 'lucide-react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'

import { Callout } from '@/components/patterns/EmptyState'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import type { SourceKind } from '@/lib/api/types'
import { cn } from '@/lib/utils'

const SOURCES: { value: SourceKind; title: string; description: string; icon: React.ReactNode }[] = [
  { value: 'git', title: 'Git repository', description: 'Build from a branch; deploy on push.', icon: <GitBranch /> },
  { value: 'image', title: 'Docker image', description: 'Pull a prebuilt image from a registry.', icon: <Box /> },
  { value: 'upload', title: 'Upload', description: 'Deploy code pushed with ferry up.', icon: <Upload /> },
]

/** Supabase-style radio cards for the service's source. */
export function SourcePicker({
  value,
  onChange,
  labelledBy,
  disabled,
}: {
  value: SourceKind
  onChange: (v: SourceKind) => void
  labelledBy: string
  disabled?: boolean
}) {
  const uid = React.useId()
  return (
    <RadioGroupPrimitive.Root
      value={value}
      onValueChange={(v) => onChange(v as SourceKind)}
      aria-labelledby={labelledBy}
      disabled={disabled}
      className="grid grid-cols-1 gap-2 sm:grid-cols-3"
    >
      {SOURCES.map((s) => (
        <RadioGroupPrimitive.Item
          key={s.value}
          value={s.value}
          aria-labelledby={`${uid}-${s.value}-t`}
          aria-describedby={`${uid}-${s.value}-d`}
          className={cn(
            'group flex cursor-pointer items-start gap-3 rounded-lg border border-border-strong bg-surface-100 p-3 text-left transition-[border-color,background-color,box-shadow] outline-none dark:bg-surface-200',
            'hover:border-border-stronger focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60',
            'data-[state=checked]:border-primary-bright/70 data-[state=checked]:bg-primary-soft/60 data-[state=checked]:ring-1 data-[state=checked]:ring-primary-bright/40',
          )}
        >
          <span
            aria-hidden="true"
            className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-lighter group-data-[state=checked]:border-primary/40 group-data-[state=checked]:text-primary [&_svg]:size-3.5"
          >
            {s.icon}
          </span>
          <span className="flex min-w-0 flex-col gap-0.5">
            <span id={`${uid}-${s.value}-t`} className="text-[13px] font-medium text-foreground">
              {s.title}
            </span>
            <span id={`${uid}-${s.value}-d`} className="text-[12px] leading-snug text-foreground-light">
              {s.description}
            </span>
          </span>
          <span
            aria-hidden="true"
            className="ml-auto flex size-4 shrink-0 items-center justify-center rounded-full border border-border-stronger group-data-[state=checked]:border-primary-bright group-data-[state=checked]:bg-primary-solid"
          >
            <span className="size-1.5 rounded-full bg-white opacity-0 group-data-[state=checked]:opacity-100" />
          </span>
        </RadioGroupPrimitive.Item>
      ))}
    </RadioGroupPrimitive.Root>
  )
}

/** Monospace one-line-by-default command field (grows with content, Enter adds lines only with Shift). */
export function CommandInput({
  className,
  onKeyDown,
  ...props
}: React.ComponentProps<typeof Textarea>) {
  return (
    <Textarea
      mono
      rows={1}
      spellCheck={false}
      autoComplete="off"
      className={cn('min-h-[34px] resize-none py-[7px]', className)}
      onKeyDown={(e) => {
        // Enter submits the form like an input; Shift+Enter inserts a new line.
        if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
          e.preventDefault()
          e.currentTarget.form?.requestSubmit()
        }
        onKeyDown?.(e)
      }}
      {...props}
    />
  )
}

/** FormCard footer: status on the left, Cancel + Save on the right. */
export function SaveFooter({
  dirty,
  saving,
  invalid,
  onCancel,
  hint,
  saveLabel = 'Save changes',
}: {
  dirty: boolean
  saving: boolean
  invalid: boolean
  onCancel: () => void
  /** Shown while pristine (e.g. "Changes apply on the next deploy"). */
  hint?: React.ReactNode
  saveLabel?: string
}) {
  return (
    <>
      <div className="mr-auto flex min-w-0 items-center gap-2 text-[13px] text-foreground-light">
        {dirty ? (
          <>
            <span aria-hidden="true" className="size-1.5 shrink-0 rounded-full bg-warning" />
            <span>
              Unsaved changes
              {invalid && <span className="text-destructive"> · check the highlighted fields</span>}
            </span>
          </>
        ) : (
          hint && <span className="hidden sm:inline">{hint}</span>
        )}
      </div>
      <Button type="button" disabled={!dirty || saving} onClick={onCancel}>
        Cancel
      </Button>
      <Button type="submit" variant="primary" disabled={!dirty} loading={saving}>
        {saveLabel}
      </Button>
    </>
  )
}

/** Shown when the server value changed while the form has unsaved edits. */
export function StaleCallout({ onLoad }: { onLoad: () => void }) {
  return (
    <div className="px-5 pt-4 md:px-6">
      <Callout
        tone="warning"
        icon={<TriangleAlert />}
        title="These settings changed elsewhere"
        actions={
          <Button size="tiny" onClick={onLoad}>
            Discard my edits and load the latest
          </Button>
        }
      >
        Saving keeps your values for the fields you edited.
      </Callout>
    </div>
  )
}

/** Error line under a form (server message after a failed save). */
export function FormError({ message }: { message: string | null }) {
  if (!message) return null
  return (
    <div className="px-5 pt-4 md:px-6">
      <Callout tone="destructive" icon={<TriangleAlert />} title="Could not save">
        <span className="break-words">{message}</span>
      </Callout>
    </div>
  )
}

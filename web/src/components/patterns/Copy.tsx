import * as React from 'react'
import { Check, Copy, Eye, EyeOff } from 'lucide-react'

import { Button, type ButtonProps } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Hint } from '@/components/ui/tooltip'
import { useCopy } from '@/hooks/useCopy'
import { cn } from '@/lib/utils'

interface CopyButtonProps extends Omit<ButtonProps, 'onClick' | 'children' | 'value'> {
  value: string
  /** Visible label ("Copy"); icon-only when omitted (then `aria-label` = "Copy <what>"). */
  label?: string
  /** What is copied, for the accessible name / tooltip ("URL", "connection string"). */
  what?: string
}

/** Tiny outlined `⧉ Copy` button. */
export function CopyButton({ value, label, what, size, variant, className, ...props }: CopyButtonProps) {
  const [copied, copy] = useCopy()
  const icon = copied ? <Check className="text-primary" /> : <Copy />
  const accessible = `Copy${what ? ` ${what}` : ''}`
  const btn = (
    <Button
      size={size ?? (label ? 'tiny' : 'icon-tiny')}
      variant={variant ?? 'default'}
      icon={icon}
      aria-label={label ? undefined : accessible}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        void copy(value)
      }}
      className={className}
      {...props}
    >
      {label && (copied ? 'Copied' : label)}
    </Button>
  )
  return label ? btn : <Hint label={copied ? 'Copied' : accessible}>{btn}</Hint>
}

interface CopyFieldProps {
  value: string
  id?: string
  /** Monospace value (ids, URLs, connection strings). */
  mono?: boolean
  what?: string
  className?: string
  size?: 'sm' | 'md'
  'aria-label'?: string
  'aria-describedby'?: string
}

/** Read-only input with an inline `Copy` button at the right end. */
export function CopyField({ value, id, mono = true, what, className, size = 'md', ...aria }: CopyFieldProps) {
  return (
    <div className={cn('relative flex w-full items-center', className)}>
      <Input
        id={id}
        readOnly
        value={value}
        mono={mono}
        size={size}
        onFocus={(e) => e.currentTarget.select()}
        className="pr-[76px] text-ellipsis"
        {...aria}
      />
      <CopyButton value={value} label="Copy" what={what} className="absolute right-1.5" />
    </div>
  )
}

interface SecretFieldProps extends CopyFieldProps {
  /** Text shown while hidden (defaults to bullets). */
  mask?: string
}

/** Read-only secret with Reveal + Copy (value never rendered until revealed). */
export function SecretField({ value, id, mono = true, what, className, size = 'md', mask, ...aria }: SecretFieldProps) {
  const [shown, setShown] = React.useState(false)
  return (
    <div className={cn('relative flex w-full items-center', className)}>
      <Input
        id={id}
        readOnly
        value={shown ? value : (mask ?? '•'.repeat(Math.min(Math.max(value.length, 12), 32)))}
        mono={mono}
        size={size}
        onFocus={(e) => shown && e.currentTarget.select()}
        className="pr-[108px] text-ellipsis"
        {...aria}
      />
      <div className="absolute right-1.5 flex items-center gap-1">
        <Hint label={shown ? 'Hide' : 'Reveal'}>
          <Button
            size="icon-tiny"
            icon={shown ? <EyeOff /> : <Eye />}
            aria-label={shown ? `Hide ${what ?? 'value'}` : `Reveal ${what ?? 'value'}`}
            aria-pressed={shown}
            onClick={() => setShown((s) => !s)}
          />
        </Hint>
        <CopyButton value={value} label="Copy" what={what} />
      </div>
    </div>
  )
}

/** Monospace snippet block (CLI commands) with a copy button. */
export function CodeBlock({ code, className, prompt = false }: { code: string; className?: string; prompt?: boolean }) {
  return (
    <div className={cn('group relative w-full rounded-md border bg-surface-200 text-left', className)}>
      <pre className="overflow-x-auto px-3 py-2.5 pr-12 font-mono text-[12.5px] leading-relaxed text-foreground">
        {prompt && <span className="text-foreground-muted select-none">$ </span>}
        {code}
      </pre>
      <CopyButton value={code} what="command" className="absolute top-1.5 right-1.5" />
    </div>
  )
}

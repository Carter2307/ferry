import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'

import { CRON_PRESETS } from './form'

const FIELD_NAMES = ['minute', 'hour', 'day', 'month', 'weekday'] as const
const FIELD_RANGES = ['0-59', '0-23', '1-31', '1-12', '0-7'] as const

interface CronFieldProps {
  id: string
  value: string
  onChange: (value: string) => void
  onBlur: () => void
  invalid?: boolean
  describedBy?: string
}

/** Cron expression input with a 5-field breakdown and common presets. */
export function CronField({ id, value, onChange, onBlur, invalid, describedBy }: CronFieldProps) {
  const trimmed = value.trim()
  const alias = trimmed.startsWith('@')
  const fields = trimmed && !alias ? trimmed.split(/\s+/) : []
  const helpId = `${id}-help`

  return (
    <div className="flex flex-col gap-2.5">
      <Input
        id={id}
        mono
        value={value}
        placeholder="*/15 * * * *"
        autoComplete="off"
        spellCheck={false}
        aria-invalid={invalid || undefined}
        aria-describedby={[helpId, describedBy].filter(Boolean).join(' ')}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
      />
      <div id={helpId} role="group" className="grid grid-cols-5 gap-1" aria-label="Schedule fields">
        {FIELD_NAMES.map((name, i) => {
          const part = alias ? undefined : fields[i]
          return (
            <div
              key={name}
              className={cn(
                'flex min-w-0 flex-col items-center gap-0.5 rounded-md border px-1 py-1.5 text-center',
                part ? 'border-border-strong bg-surface-200' : 'border-dashed',
              )}
            >
              <span className="max-w-full truncate font-mono text-[12.5px] text-foreground">{part ?? '·'}</span>
              <span className="font-mono text-[10px] tracking-[0.06em] text-foreground-lighter uppercase">{name}</span>
              <span className="sr-only">range {FIELD_RANGES[i]}</span>
            </div>
          )
        })}
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {CRON_PRESETS.map((p) => (
          <Button
            key={p.value}
            size="tiny"
            variant={trimmed === p.value ? 'outline' : 'ghost'}
            aria-pressed={trimmed === p.value}
            className={cn(trimmed === p.value && 'border-primary/50 text-primary')}
            onClick={() => onChange(p.value)}
          >
            {p.label}
          </Button>
        ))}
      </div>
    </div>
  )
}

import { Globe, Plus, X } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

import { parseDomains } from './form'

interface DomainsInputProps {
  id: string
  value: string[]
  onChange: (domains: string[]) => void
  /** Text typed but not yet added (validated by the form). */
  draft: string
  onDraftChange: (draft: string) => void
  /** Called when adding the draft failed (so the form shows its error). */
  onInvalidDraft: () => void
  invalid?: boolean
  describedBy?: string
}

/**
 * Custom domains as removable chips: type a domain and press Enter (or `,`),
 * or paste a list. Each one is validated like `validate::domain`.
 */
export function DomainsInput({ id, value, onChange, draft, onDraftChange, onInvalidDraft, invalid, describedBy }: DomainsInputProps) {
  const commit = (text = draft) => {
    if (!text.trim()) return
    const parsed = parseDomains(text)
    if ('error' in parsed) {
      onInvalidDraft()
      return
    }
    onChange([...value, ...parsed.domains.filter((d) => !value.includes(d))])
    onDraftChange('')
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-2">
        <Input
          id={id}
          mono
          value={draft}
          placeholder="app.example.com"
          autoComplete="off"
          spellCheck={false}
          aria-invalid={invalid || undefined}
          aria-describedby={describedBy}
          onChange={(e) => onDraftChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ',') {
              e.preventDefault()
              commit()
            } else if (e.key === 'Backspace' && draft === '' && value.length > 0) {
              onChange(value.slice(0, -1))
            }
          }}
          onBlur={() => commit()}
          onPaste={(e) => {
            const text = e.clipboardData.getData('text')
            if (/[\s,]/.test(text.trim())) {
              e.preventDefault()
              commit(`${draft} ${text}`)
            }
          }}
        />
        <Button type="button" size="md" icon={<Plus />} onClick={() => commit()} disabled={!draft.trim()}>
          Add
        </Button>
      </div>
      {value.length > 0 && (
        <ul className="flex flex-wrap gap-1.5" aria-label="Custom domains">
          {value.map((d) => (
            <li
              key={d}
              className="inline-flex h-7 max-w-full items-center gap-1.5 rounded-md border border-border-strong bg-surface-200 pr-1 pl-2 font-mono text-[12.5px] text-foreground"
            >
              <Globe className="size-3.5 shrink-0 text-foreground-lighter" aria-hidden="true" />
              <span className="truncate">{d}</span>
              <button
                type="button"
                onClick={() => onChange(value.filter((x) => x !== d))}
                className="flex size-5 shrink-0 items-center justify-center rounded-sm text-foreground-lighter outline-none hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                aria-label={`Remove ${d}`}
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

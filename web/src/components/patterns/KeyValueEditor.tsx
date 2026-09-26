import * as React from 'react'
import { Eye, EyeOff, FileUp, Plus, Trash2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Hint } from '@/components/ui/tooltip'
import type { EnvVar } from '@/lib/api/types'
import { parseDotenv } from '@/lib/dotenv'
import { cn } from '@/lib/utils'

import { mergeRows, newRowId, validateRows, type KvRow } from './kv-rows'
import { MonoLabel } from './MonoLabel'

interface KeyValueEditorProps {
  rows: KvRow[]
  onChange: (rows: KvRow[]) => void
  /** Validation messages from `validateRows` (computed if omitted). */
  errors?: Map<string, string>
  readOnly?: boolean
  /** Mask values (password inputs) with a per-row reveal toggle. */
  maskValues?: boolean
  keyPlaceholder?: string
  valuePlaceholder?: string
  addLabel?: string
  /** Show the "Import .env" button. Default true. */
  allowImport?: boolean
  className?: string
}

/**
 * Environment variable editor: KEY / VALUE rows, add / remove, reveal,
 * `.env` import (dialog or paste `KEY=VALUE` lines straight into a key field).
 */
export function KeyValueEditor({
  rows,
  onChange,
  errors: errorsProp,
  readOnly = false,
  maskValues = true,
  keyPlaceholder = 'KEY',
  valuePlaceholder = 'value',
  addLabel = 'Add variable',
  allowImport = true,
  className,
}: KeyValueEditorProps) {
  const errors = React.useMemo(() => errorsProp ?? validateRows(rows), [errorsProp, rows])
  const [revealed, setRevealed] = React.useState<Set<string>>(() => new Set())
  const [importOpen, setImportOpen] = React.useState(false)
  // id of a just-added row whose key input should take focus once mounted
  const focusIdRef = React.useRef<string | null>(null)

  const update = (id: string, patch: Partial<KvRow>) => onChange(rows.map((r) => (r.id === id ? { ...r, ...patch } : r)))
  const remove = (id: string) => onChange(rows.filter((r) => r.id !== id))
  const add = () => {
    const id = newRowId()
    focusIdRef.current = id
    onChange([...rows, { id, key: '', value: '' }])
  }
  const toggleReveal = (id: string) =>
    setRevealed((s) => {
      const n = new Set(s)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })

  const onKeyPaste = (e: React.ClipboardEvent<HTMLInputElement>, row: KvRow) => {
    const text = e.clipboardData.getData('text')
    if (!text.includes('=') && !text.includes('\n')) return
    const { vars } = parseDotenv(text)
    if (vars.length === 0) return
    e.preventDefault()
    const without = row.key === '' && row.value === '' ? rows.filter((r) => r.id !== row.id) : rows
    onChange(mergeRows(without, vars))
  }

  return (
    <div className={cn('flex flex-col gap-2', className)}>
      {rows.length > 0 && (
        <div className="hidden grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] gap-2 px-0.5 sm:grid" aria-hidden="true">
          <MonoLabel>Key</MonoLabel>
          <MonoLabel>Value</MonoLabel>
          <span className={cn(readOnly ? 'w-0' : maskValues ? 'w-[60px]' : 'w-[30px]')} />
        </div>
      )}
      <ul className="flex flex-col gap-2" aria-label="Environment variables">
        {rows.map((row, i) => {
          const err = errors.get(row.id)
          const shown = !maskValues || revealed.has(row.id)
          const errId = `${row.id}-err`
          return (
            <li key={row.id} className="flex flex-col gap-1">
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto]">
                <Input
                  ref={(el) => {
                    if (el && focusIdRef.current === row.id) {
                      focusIdRef.current = null
                      el.focus()
                    }
                  }}
                  mono
                  value={row.key}
                  placeholder={keyPlaceholder}
                  readOnly={readOnly}
                  aria-label={`Key ${i + 1}`}
                  aria-invalid={err ? true : undefined}
                  aria-describedby={err ? errId : undefined}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(e) => update(row.id, { key: e.target.value })}
                  onPaste={(e) => onKeyPaste(e, row)}
                />
                <div className="flex gap-2">
                  <Input
                    mono
                    type={shown ? 'text' : 'password'}
                    value={row.value}
                    placeholder={valuePlaceholder}
                    readOnly={readOnly}
                    aria-label={`Value of ${row.key || `variable ${i + 1}`}`}
                    autoComplete="off"
                    spellCheck={false}
                    onChange={(e) => update(row.id, { value: e.target.value })}
                  />
                  {!readOnly && (
                    <div className="flex shrink-0 items-center gap-1">
                      {maskValues && (
                        <Hint label={shown ? 'Hide value' : 'Reveal value'}>
                          <Button
                            size="icon-md"
                            variant="ghost"
                            icon={shown ? <EyeOff /> : <Eye />}
                            aria-label={shown ? `Hide value of ${row.key || `variable ${i + 1}`}` : `Reveal value of ${row.key || `variable ${i + 1}`}`}
                            aria-pressed={shown}
                            onClick={() => toggleReveal(row.id)}
                          />
                        </Hint>
                      )}
                      <Hint label="Remove">
                        <Button
                          size="icon-md"
                          variant="ghost"
                          icon={<Trash2 />}
                          aria-label={`Remove ${row.key || `variable ${i + 1}`}`}
                          onClick={() => remove(row.id)}
                        />
                      </Hint>
                    </div>
                  )}
                  {readOnly && maskValues && (
                    <Button
                      size="icon-md"
                      variant="ghost"
                      icon={shown ? <EyeOff /> : <Eye />}
                      aria-label={shown ? 'Hide value' : 'Reveal value'}
                      aria-pressed={shown}
                      onClick={() => toggleReveal(row.id)}
                    />
                  )}
                </div>
              </div>
              {err && (
                <p id={errId} role="alert" className="text-[12.5px] text-destructive">
                  {err}
                </p>
              )}
            </li>
          )
        })}
      </ul>
      {!readOnly && (
        <div className="flex flex-wrap items-center gap-2 pt-1">
          <Button size="tiny" icon={<Plus />} onClick={add}>
            {addLabel}
          </Button>
          {allowImport && (
            <Button size="tiny" variant="ghost" icon={<FileUp />} onClick={() => setImportOpen(true)}>
              Import .env
            </Button>
          )}
          <span className="hidden text-[12px] text-foreground-lighter md:inline">
            Tip: paste <code className="font-mono">KEY=value</code> lines into a key field.
          </span>
        </div>
      )}
      <ImportDotenvDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        onImport={(vars) => onChange(mergeRows(rows, vars))}
      />
    </div>
  )
}

function ImportDotenvDialog({
  open,
  onOpenChange,
  onImport,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onImport: (vars: EnvVar[]) => void
}) {
  const [text, setText] = React.useState('')
  const parsed = React.useMemo(() => parseDotenv(text), [text])
  const textareaId = React.useId()
  const readFile = async (file: File) => setText(await file.text())

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        onOpenChange(o)
        if (!o) setText('')
      }}
    >
      <DialogContent size="xl">
        <DialogHeader>
          <DialogTitle>Import from .env</DialogTitle>
          <DialogDescription>
            Paste the contents of a <code className="font-mono">.env</code> file. Existing keys are overwritten.
          </DialogDescription>
        </DialogHeader>
        <DialogBody>
          <label htmlFor={textareaId} className="sr-only">
            .env contents
          </label>
          <Textarea
            id={textareaId}
            mono
            rows={10}
            className="max-h-[50dvh] min-h-48"
            placeholder={'DATABASE_URL=postgres://…\nAPI_KEY="secret"\n# comments are ignored'}
            value={text}
            onChange={(e) => setText(e.target.value)}
            spellCheck={false}
          />
          <div className="flex flex-wrap items-center justify-between gap-2 text-[13px] text-foreground-light">
            <span>
              {parsed.vars.length} variable{parsed.vars.length === 1 ? '' : 's'} detected
              {parsed.invalid.length > 0 && (
                <span className="text-warning"> · {parsed.invalid.length} line(s) skipped</span>
              )}
            </span>
            <label className="cursor-pointer text-primary underline-offset-4 hover:underline">
              Choose file…
              <input
                type="file"
                accept=".env,text/plain"
                className="sr-only"
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  if (f) void readFile(f)
                }}
              />
            </label>
          </div>
        </DialogBody>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button
            variant="primary"
            disabled={parsed.vars.length === 0}
            onClick={() => {
              onImport(parsed.vars)
              onOpenChange(false)
              setText('')
            }}
          >
            Import {parsed.vars.length > 0 ? parsed.vars.length : ''} variable{parsed.vars.length === 1 ? '' : 's'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

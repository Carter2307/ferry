import * as React from 'react'
import { AlertTriangle, BookOpen, Eraser, FileCode2, FilePlus2, FolderOpen, ListChecks, Rocket } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Kbd } from '@/components/patterns/Kbd'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageContainer, PageHeader } from '@/components/patterns/Page'
import { DESIGN_DOC_URL } from '@/components/shell/nav'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Hint } from '@/components/ui/tooltip'
import { errorMessage } from '@/lib/api/client'
import { useApplyBlueprint } from '@/lib/api/queries'
import type { BlueprintResult } from '@/lib/api/types'
import { plural } from '@/lib/format'
import { isMac } from '@/lib/platform'
import { cn } from '@/lib/utils'

import { BlueprintResultView } from './BlueprintResultView'
import { loadDraft, saveDraft } from './draft'
import { countActions, errorLocation, EXAMPLE_BLUEPRINT, MAX_BLUEPRINT_BYTES } from './lib'
import { YamlEditor, type YamlEditorHandle } from './YamlEditor'

const BLUEPRINT_DOCS_URL = `${DESIGN_DOC_URL}#11-blueprints-ferryyaml--renderyaml`

interface Run {
  /** YAML the result was computed for (a later edit makes it stale). */
  yaml: string
  result: BlueprintResult
}

interface RunError {
  message: string
  line: number | null
  column: number | null
}

const STEPS = [
  { title: 'Load or write a blueprint', text: 'Open ferry.yaml or render.yaml, or start from the example.' },
  { title: 'Dry run', text: 'Preview what would be created and updated, with warnings for unsupported keys.' },
  { title: 'Apply', text: 'Write the changes. New and changed services are deployed; env-only changes restart them.' },
]

function HowItWorks() {
  return (
    <Card className="gap-0">
      <div className="border-b px-5 py-4">
        <p className="text-sm font-medium text-foreground">How it works</p>
        <p className="text-[13px] text-foreground-light">Render’s blueprint format, a pragmatic subset.</p>
      </div>
      <ol className="divide-y">
        {STEPS.map((s, i) => (
          <li key={s.title} className="flex gap-3 px-5 py-3.5">
            <span
              aria-hidden="true"
              className="mt-px flex size-5 shrink-0 items-center justify-center rounded-full border border-border-stronger font-mono text-[11px] text-foreground-light"
            >
              {i + 1}
            </span>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="text-[13px] font-medium text-foreground">{s.title}</span>
              <span className="text-[12.5px] leading-relaxed text-foreground-light">{s.text}</span>
            </span>
          </li>
        ))}
      </ol>
      <div className="flex flex-col gap-2 border-t px-5 py-4">
        <MonoLabel>Top-level keys</MonoLabel>
        <ul className="flex flex-wrap gap-1.5">
          {['services', 'databases', 'envVarGroups'].map((k) => (
            <li
              key={k}
              className="rounded-sm border border-border-strong bg-surface-200 px-1.5 py-0.5 font-mono text-[11.5px] text-foreground-light"
            >
              {k}
            </li>
          ))}
        </ul>
        <p className="text-[12.5px] leading-relaxed text-foreground-lighter">
          Existing resources are updated in place; nothing is ever deleted.
        </p>
      </div>
    </Card>
  )
}

/** `/blueprints` — edit / load a blueprint, dry run it, then apply. */
export function BlueprintsPage() {
  const initial = React.useMemo(() => loadDraft(), [])
  const [yaml, setYaml] = React.useState(initial?.yaml ?? '')
  const [fileName, setFileName] = React.useState<string | null>(initial?.fileName ?? null)
  const [run, setRun] = React.useState<Run | null>(null)
  const [runError, setRunError] = React.useState<RunError | null>(null)
  const [confirmApply, setConfirmApply] = React.useState(false)
  const [dragging, setDragging] = React.useState(false)
  const [pending, setPending] = React.useState<'dry' | 'apply' | null>(null)
  const apply = useApplyBlueprint()
  const editorRef = React.useRef<YamlEditorHandle | null>(null)
  const fileInputRef = React.useRef<HTMLInputElement | null>(null)
  const planRef = React.useRef<HTMLDivElement | null>(null)
  // Set by a successful dry run: bring the plan (rendered below the editor) into view.
  const revealPlan = React.useRef(false)
  const editorId = React.useId()
  const hintId = React.useId()

  // Persist the draft (debounced) for this tab.
  React.useEffect(() => {
    const t = setTimeout(() => saveDraft({ yaml, fileName }), 300)
    return () => clearTimeout(t)
  }, [yaml, fileName])

  const empty = yaml.trim() === ''
  const stale = run !== null && run.yaml !== yaml
  const counts = run ? countActions(run.result.actions) : null
  const changes = counts ? counts.create + counts.update : 0
  const canApply = run !== null && run.result.dry_run && !stale && changes > 0

  const execute = async (dryRun: boolean) => {
    if (empty || pending) return
    const sent = yaml
    setPending(dryRun ? 'dry' : 'apply')
    try {
      const result = await apply.mutateAsync({ yaml: sent, dry_run: dryRun })
      setRun({ yaml: sent, result })
      setRunError(null)
      if (dryRun) {
        revealPlan.current = true
        const c = countActions(result.actions)
        const parts = [
          c.create > 0 && `${c.create} to create`,
          c.update > 0 && `${c.update} to update`,
          c.unchanged > 0 && `${c.unchanged} unchanged`,
        ].filter(Boolean)
        toast.info('Plan ready', {
          description: `${parts.join(' · ') || 'Nothing to change'}${
            result.warnings.length > 0 ? ` · ${plural(result.warnings.length, 'warning')}` : ''
          }`,
        })
      }
      if (!dryRun) {
        const c = countActions(result.actions)
        toast.success('Blueprint applied', {
          description: `${c.create} created · ${c.update} updated · ${plural(result.deploys.length, 'deploy')} queued`,
        })
      }
    } catch (e) {
      const message = errorMessage(e)
      const loc = errorLocation(message)
      setRunError({ message, line: loc?.line ?? null, column: loc?.column ?? null })
      setRun(null)
      if (!dryRun) throw e
    } finally {
      setPending(null)
    }
  }

  React.useEffect(() => {
    if (!run || !revealPlan.current) return
    revealPlan.current = false
    const el = planRef.current
    if (!el) return
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    el.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' })
    el.focus({ preventScroll: true })
  }, [run])

  const loadFile = async (file: File) => {
    if (file.size > MAX_BLUEPRINT_BYTES) {
      toast.error(`${file.name} is too large`, { description: 'Blueprints are limited to 1 MiB.' })
      return
    }
    try {
      const text = await file.text()
      setYaml(text.replace(/\r\n/g, '\n'))
      setFileName(file.name)
      setRun(null)
      setRunError(null)
      toast.success(`Loaded ${file.name}`, { description: plural(text.split('\n').length, 'line') })
    } catch (e) {
      toast.error(`Could not read ${file.name}`, { description: errorMessage(e) })
    }
  }

  const clear = () => {
    const previous = { yaml, fileName }
    setYaml('')
    setFileName(null)
    setRun(null)
    setRunError(null)
    toast('Editor cleared', {
      action: {
        label: 'Undo',
        onClick: () => {
          setYaml(previous.yaml)
          setFileName(previous.fileName)
        },
      },
    })
  }

  const insertExample = () => {
    const previous = { yaml, fileName }
    setYaml(EXAMPLE_BLUEPRINT)
    setFileName('ferry.yaml')
    setRun(null)
    setRunError(null)
    if (previous.yaml.trim() !== '') {
      toast('Replaced the editor with the example', {
        action: {
          label: 'Undo',
          onClick: () => {
            setYaml(previous.yaml)
            setFileName(previous.fileName)
          },
        },
      })
    }
  }

  const lineCount = yaml === '' ? 0 : yaml.split('\n').length

  return (
    <PageContainer>
      <PageHeader
        title="Blueprints"
        description="Describe services, datastores and env groups in a ferry.yaml (or Render’s render.yaml) and apply them in one go."
        actions={
          <Button asChild icon={<BookOpen />}>
            <a href={BLUEPRINT_DOCS_URL} target="_blank" rel="noreferrer">
              Blueprint reference
            </a>
          </Button>
        }
      />

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_300px]">
        <Card className="min-w-0 gap-0">
          {/* header: file + tools */}
          <div className="flex flex-col gap-3 border-b px-4 py-3 sm:flex-row sm:items-center sm:justify-between md:px-5">
            <div className="flex min-w-0 items-center gap-2.5">
              <FileCode2 className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
              <span className="truncate font-mono text-[13px] text-foreground">{fileName ?? 'untitled.yaml'}</span>
              <span className="shrink-0 text-[12px] text-foreground-lighter">
                {lineCount > 0 ? plural(lineCount, 'line') : 'empty'}
                {!empty && ' · draft kept in this tab'}
              </span>
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-1.5">
              <input
                ref={fileInputRef}
                type="file"
                accept=".yaml,.yml,application/yaml,text/yaml,application/x-yaml"
                className="sr-only"
                tabIndex={-1}
                aria-hidden="true"
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  if (f) void loadFile(f)
                  e.target.value = ''
                }}
              />
              <Button size="tiny" icon={<FolderOpen />} onClick={() => fileInputRef.current?.click()}>
                Open file…
              </Button>
              <Button
                size="tiny"
                variant="ghost"
                icon={<FilePlus2 />}
                onClick={insertExample}
                disabled={yaml === EXAMPLE_BLUEPRINT}
              >
                Example
              </Button>
              <Button
                size="tiny"
                variant="ghost"
                icon={<Eraser />}
                onClick={clear}
                disabled={yaml === '' && fileName === null}
              >
                Clear
              </Button>
            </div>
          </div>

          {/* editor (also a drop target for .yaml files) */}
          <div
            className={cn('relative p-3 md:p-4', dragging && 'bg-primary-soft')}
            onDragOver={(e) => {
              if (!e.dataTransfer.types.includes('Files')) return
              e.preventDefault()
              setDragging(true)
            }}
            onDragLeave={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false)
            }}
            onDrop={(e) => {
              const f = e.dataTransfer.files[0]
              if (!f) return
              e.preventDefault()
              setDragging(false)
              void loadFile(f)
            }}
          >
            <label htmlFor={editorId} className="sr-only">
              Blueprint YAML
            </label>
            <YamlEditor
              ref={editorRef}
              id={editorId}
              value={yaml}
              onChange={setYaml}
              errorLine={runError?.line ?? null}
              onSubmitShortcut={() => void execute(true)}
              placeholder={
                'services:\n  - type: web\n    name: my-app\n    image: { url: nginx:alpine }\n\nPaste a blueprint, open ferry.yaml / render.yaml, or drop a file here.'
              }
              aria-describedby={hintId}
              className="h-[min(56vh,520px)] min-h-[300px]"
            />
            {dragging && (
              <div className="pointer-events-none absolute inset-3 flex items-center justify-center rounded-md border-2 border-dashed border-primary-bright/70 text-sm font-medium text-primary md:inset-4">
                Drop to open the file
              </div>
            )}
          </div>

          {runError && (
            <div
              role="alert"
              className="mx-3 mb-3 flex flex-col gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] sm:flex-row sm:items-start md:mx-4 md:mb-4"
            >
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <p className="font-medium text-foreground">The blueprint was rejected</p>
                <p className="font-mono text-[12.5px] break-words text-foreground-light">{runError.message}</p>
              </div>
              {runError.line !== null && (
                <Button
                  size="tiny"
                  className="shrink-0"
                  onClick={() => runError.line !== null && editorRef.current?.goToLine(runError.line)}
                >
                  Go to line {runError.line}
                </Button>
              )}
            </div>
          )}

          {/* footer */}
          <div className="flex flex-col gap-3 border-t bg-surface-75 px-4 py-3 sm:flex-row sm:items-center sm:justify-between md:px-5 dark:bg-transparent">
            <p id={hintId} className="text-[12px] text-foreground-lighter">
              <Kbd>{isMac ? '⌘' : 'Ctrl'}</Kbd> <Kbd>↵</Kbd> dry run · <Kbd>Tab</Kbd> indents · <Kbd>Esc</Kbd> then{' '}
              <Kbd>Tab</Kbd> leaves the editor
            </p>
            <div className="flex shrink-0 items-center justify-end gap-2">
              <Button
                icon={<ListChecks />}
                disabled={empty || pending === 'apply'}
                loading={pending === 'dry'}
                onClick={() => void execute(true)}
              >
                Dry run
              </Button>
              <Hint
                label={
                  empty
                    ? 'Write a blueprint first'
                    : !run || stale
                      ? 'Run a dry run of the current blueprint first'
                      : !run.result.dry_run
                        ? 'Already applied'
                        : changes === 0
                          ? 'Nothing to change'
                          : `Apply ${plural(changes, 'change')}`
                }
              >
                {/* span keeps the tooltip working while the button is disabled */}
                <span
                  tabIndex={canApply ? -1 : 0}
                  className="inline-flex rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <Button
                    variant="primary"
                    icon={<Rocket />}
                    disabled={!canApply || pending !== null}
                    loading={pending === 'apply'}
                    onClick={() => setConfirmApply(true)}
                  >
                    Apply
                  </Button>
                </span>
              </Hint>
            </div>
          </div>
        </Card>

        <aside className="hidden xl:block">
          <HowItWorks />
        </aside>
      </div>

      {run && (
        <div
          ref={planRef}
          tabIndex={-1}
          role="region"
          aria-label="Blueprint plan"
          className="mt-10 scroll-mt-6 rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <BlueprintResultView result={run.result} stale={stale} />
        </div>
      )}

      <ConfirmDialog
        open={confirmApply}
        onOpenChange={setConfirmApply}
        variant="primary"
        title="Apply this blueprint?"
        description={
          counts && (
            <>
              <p>
                {counts.create > 0 && (
                  <>
                    <strong className="font-medium text-foreground">{counts.create} to create</strong>
                    {counts.update > 0 ? ', ' : ''}
                  </>
                )}
                {counts.update > 0 && (
                  <strong className="font-medium text-foreground">{counts.update} to update</strong>
                )}
                {counts.unchanged > 0 && <>, {counts.unchanged} unchanged</>}.
              </p>
              <p>
                New services with a source get their first deploy, services whose build settings changed are redeployed,
                and live services whose variables changed are restarted.
              </p>
              {run && run.result.warnings.length > 0 && (
                <p className="text-warning">
                  Review the {plural(run.result.warnings.length, 'warning')} of the dry run first.
                </p>
              )}
            </>
          )
        }
        confirmLabel="Apply blueprint"
        onConfirm={() => execute(false)}
      />
    </PageContainer>
  )
}

import { Check, Copy, Eye, EyeOff, TriangleAlert } from 'lucide-react'

import { Callout, ErrorState } from '@/components/patterns/EmptyState'
import { FormCard } from '@/components/patterns/FormCard'
import { KeyValueEditor } from '@/components/patterns/KeyValueEditor'
import type { KvRow } from '@/components/patterns/kv-rows'
import { VariablesFooter } from '@/components/patterns/VariablesFooter'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Hint } from '@/components/ui/tooltip'
import { useCopy } from '@/hooks/useCopy'
import type { EnvVar } from '@/lib/api/types'
import { toDotenv } from '@/lib/dotenv'

export interface ServiceVariablesCardProps {
  serviceName: string
  rows: KvRow[]
  onRowsChange: (rows: KvRow[]) => void
  errors: Map<string, string>
  vars: EnvVar[]
  dirty: boolean
  valid: boolean
  loading: boolean
  loadError: unknown
  onRetry: () => void
  retrying: boolean
  /** A newer server version arrived while editing. */
  remoteChanged: boolean
  onLoadRemote: () => void
  /** Offer "Save & restart" (service has a live deploy and is not suspended). */
  canRestart: boolean
  saving: 'save' | 'restart' | null
  onSave: (restart: boolean) => void
  onCancel: () => void
  revealed: boolean
  onToggleReveal: () => void
}

/**
 * The service's own variables: KeyValueEditor in a FormCard with a
 * Cancel / Save / Save & restart footer.
 */
export function ServiceVariablesCard({
  serviceName,
  rows,
  onRowsChange,
  errors,
  vars,
  dirty,
  valid,
  loading,
  loadError,
  onRetry,
  retrying,
  remoteChanged,
  onLoadRemote,
  canRestart,
  saving,
  onSave,
  onCancel,
  revealed,
  onToggleReveal,
}: ServiceVariablesCardProps) {
  const [copied, copy] = useCopy()
  const busy = saving !== null
  const ready = !loading && !loadError

  return (
    <FormCard
      id="service-variables"
      aria-label="Service variables"
      title="Service variables"
      description="Set on this service only. They override variables with the same key from linked groups."
      headerActions={
        ready && rows.length > 0 ? (
          <>
            <Hint label={revealed ? 'Mask values' : 'Reveal all values'}>
              <Button
                size="tiny"
                variant="ghost"
                icon={revealed ? <EyeOff /> : <Eye />}
                aria-pressed={revealed}
                onClick={onToggleReveal}
              >
                {revealed ? 'Hide' : 'Reveal'}
              </Button>
            </Hint>
            <Button
              size="tiny"
              icon={copied ? <Check className="text-primary" /> : <Copy />}
              disabled={vars.length === 0}
              onClick={() => void copy(toDotenv(vars) + '\n')}
            >
              {copied ? 'Copied' : 'Copy .env'}
            </Button>
          </>
        ) : undefined
      }
      onSubmit={(e) => {
        e.preventDefault()
        // Enter in a field saves without restarting (restart is an explicit click).
        if (dirty && valid && !busy) onSave(false)
      }}
      footer={
        ready ? (
          <VariablesFooter
            dirty={dirty}
            valid={valid}
            count={vars.length}
            canRestart={canRestart}
            restartHint={`Saves, then restarts ${serviceName}`}
            saving={saving}
            onCancel={onCancel}
            onSaveAndRestart={() => onSave(true)}
          />
        ) : undefined
      }
    >
      <div className="flex flex-col gap-4 px-5 py-5 md:px-6">
        {loading ? (
          <div className="flex flex-col gap-2" aria-busy="true" aria-label="Loading variables">
            {[0, 1, 2].map((i) => (
              <div key={i} className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,2fr)_minmax(0,3fr)_68px]">
                <Skeleton className="h-[34px]" />
                <Skeleton className="h-[34px]" />
                <Skeleton className="hidden h-[34px] sm:block" />
              </div>
            ))}
          </div>
        ) : loadError ? (
          <ErrorState error={loadError} title="Could not load the variables" onRetry={onRetry} retrying={retrying} />
        ) : (
          <>
            {remoteChanged && (
              <Callout
                tone="warning"
                icon={<TriangleAlert />}
                title="The variables changed elsewhere"
                actions={
                  <Button size="tiny" onClick={onLoadRemote}>
                    Discard my edits and load the latest
                  </Button>
                }
              >
                Someone updated {serviceName}’s variables while you were editing. Saving replaces them with the list below.
              </Callout>
            )}
            {rows.length === 0 && (
              <p className="text-[13px] text-foreground-light">
                No variables yet. Add one, paste <code className="font-mono text-foreground">KEY=value</code> lines into a key
                field, or import a <code className="font-mono text-foreground">.env</code> file.
              </p>
            )}
            {/* disabled while saving: the saved list must be the one on screen */}
            <fieldset disabled={busy} className="m-0 min-w-0 border-0 p-0">
              <legend className="sr-only">Variables of {serviceName}</legend>
              <KeyValueEditor
                rows={rows}
                onChange={onRowsChange}
                errors={errors}
                maskValues={!revealed}
                valuePlaceholder="value or ${{datastore.NAME.connectionString}}"
              />
            </fieldset>
          </>
        )}
      </div>
    </FormCard>
  )
}

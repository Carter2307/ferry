import * as React from 'react'
import { Eye, EyeOff, Link2, TriangleAlert } from 'lucide-react'

import { TableErrorRow, TableMessageRow, TableSkeletonRows } from '@/components/patterns/DataTable'
import { PageSection } from '@/components/patterns/Page'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Hint } from '@/components/ui/tooltip'
import type { EnvGroupView, EnvVar, ServiceView } from '@/lib/api/types'
import { cn } from '@/lib/utils'

import {
  findReferences,
  injectedVars,
  mergeLayers,
  PER_DEPLOY,
  type EffectiveVar,
  type EnvLayer,
  type KnownTargets,
} from './effective'

interface EffectiveEnvTableProps {
  service: ServiceView
  /** The service's own variables (the editor's current, possibly unsaved, list). */
  ownVars: EnvVar[]
  dirty: boolean
  groups: EnvGroupView[] | undefined
  loading: boolean
  /** The service's own variables could not be loaded (the merged view would be wrong). */
  error?: unknown
  known: KnownTargets | null
}

const MASK = '••••••••••••'

function SourcePill({ layer }: { layer: EnvLayer }) {
  if (layer.kind === 'service') return <Badge variant="success">Service</Badge>
  if (layer.kind === 'ferry') return <Badge variant="outline">Ferry</Badge>
  return (
    <Badge variant="info" case="normal" font="mono" className="max-w-40">
      <span className="truncate">{layer.label}</span>
    </Badge>
  )
}

/** Value with `${{…}}` references highlighted (references are never secret). */
function ValueCell({ item, revealed, known }: { item: EffectiveVar; revealed: boolean; known: KnownTargets | null }) {
  const refs = React.useMemo(() => findReferences(item.value, known), [item.value, known])
  const problems = refs.filter((r) => r.problem)
  const isRefOnly = refs.length > 0 && refs.map((r) => r.raw).join('') === item.value.trim()
  const perDeploy = item.source.kind === 'ferry' && item.value === PER_DEPLOY
  const show = revealed || isRefOnly || item.source.kind === 'ferry'

  return (
    <div className="flex min-w-0 flex-col items-start gap-1.5 md:flex-row md:items-center md:gap-2">
      {perDeploy ? (
        <span className="text-[13px] text-foreground-lighter italic">set at each deploy</span>
      ) : item.value === '' ? (
        <span className="text-[13px] text-foreground-lighter italic">empty</span>
      ) : (
        <code
          className={cn(
            'min-w-0 font-mono text-[12.5px] break-all',
            show ? 'text-foreground' : 'tracking-wider text-foreground-lighter select-none',
          )}
        >
          {show ? <Highlighted value={item.value} /> : MASK}
        </code>
      )}
      {refs.length > 0 && problems.length === 0 && (
        <Hint label="Resolved when a container starts">
          <span className="inline-flex shrink-0">
            <Badge variant="outline" className="gap-1" tabIndex={0}>
              <Link2 aria-hidden="true" />
              Ref
            </Badge>
          </span>
        </Hint>
      )}
      {problems.length > 0 && (
        <Hint label={problems.map((p) => p.problem).join(' · ')}>
          <span className="inline-flex shrink-0">
            <Badge variant="warning" className="gap-1" tabIndex={0} aria-label={`Reference problem: ${problems.map((p) => p.problem).join('; ')}`}>
              <TriangleAlert aria-hidden="true" />
              Check ref
            </Badge>
          </span>
        </Hint>
      )}
    </div>
  )
}

/** "overrides ~~shared~~, ~~ferry~~" under the winning source. */
function OverrideNote({ item }: { item: EffectiveVar }) {
  if (item.overridden.length === 0) return null
  return (
    <span className="text-[12px] leading-snug text-foreground-lighter">
      overrides{' '}
      {item.overridden.map((l, i) => (
        <React.Fragment key={l.id}>
          {i > 0 && ', '}
          <span className="line-through decoration-foreground-muted">
            {l.kind === 'group' ? l.label : l.kind === 'ferry' ? 'Ferry' : 'service'}
          </span>
        </React.Fragment>
      ))}
    </span>
  )
}

/** Splits a value into plain text and `${{…}}` spans. */
function Highlighted({ value }: { value: string }) {
  const parts: React.ReactNode[] = []
  const re = /\$?\$\{\{[^}]*\}\}/g
  let last = 0
  let m: RegExpExecArray | null
  let i = 0
  while ((m = re.exec(value)) !== null) {
    if (m.index > last) parts.push(value.slice(last, m.index))
    const text = m[0]
    parts.push(
      text.startsWith('$$') ? (
        text
      ) : (
        <span key={i++} className="rounded-sm bg-primary-soft px-0.5 text-primary">
          {text}
        </span>
      ),
    )
    last = m.index + text.length
  }
  if (last < value.length) parts.push(value.slice(last))
  return <>{parts}</>
}

/**
 * Read-only merged view: Ferry-injected variables < linked groups (in link
 * order) < the service's own variables, with the winning layer per key.
 */
export function EffectiveEnvTable({ service, ownVars, dirty, groups, loading, error, known }: EffectiveEnvTableProps) {
  const [showInjected, setShowInjected] = React.useState(false)
  const [revealed, setRevealed] = React.useState(false)
  const switchId = React.useId()

  const layers = React.useMemo<EnvLayer[]>(() => {
    const byName = new Map((groups ?? []).map((g) => [g.name, g]))
    const out: EnvLayer[] = []
    if (showInjected) out.push({ id: 'ferry', kind: 'ferry', label: 'Ferry', vars: injectedVars(service) })
    for (const g of service.env_groups) {
      out.push({ id: `group:${g}`, kind: 'group', label: g, vars: byName.get(g)?.vars ?? [] })
    }
    out.push({ id: 'service', kind: 'service', label: 'This service', vars: ownVars })
    return out
  }, [groups, ownVars, service, showInjected])

  const merged = React.useMemo(() => mergeLayers(layers), [layers])
  const injected = React.useMemo(() => injectedVars(service), [service])
  const injectedCount = injected.length
  const injectedExamples = (injected.some((v) => v.key === 'PORT') ? ['PORT', 'FERRY_SERVICE_NAME'] : ['FERRY_SERVICE_NAME', 'FERRY_DEPLOY_ID']).join(', ')

  return (
    <PageSection
      title={
        <span className="flex flex-wrap items-center gap-2">
          Effective environment
          {dirty && (
            <Badge variant="warning" title="Includes changes that are not saved yet">
              Unsaved preview
            </Badge>
          )}
        </span>
      }
      description="What the next deploy or restart gives the containers, before references are resolved. The service's own variables win, then the last linked group."
      actions={
        <>
          <label htmlFor={switchId} className="flex cursor-pointer items-center gap-2 text-[13px] text-foreground-light">
            <Switch id={switchId} size="sm" checked={showInjected} onCheckedChange={setShowInjected} />
            Show Ferry variables
          </label>
          <Button
            size="tiny"
            variant="ghost"
            icon={revealed ? <EyeOff /> : <Eye />}
            aria-pressed={revealed}
            onClick={() => setRevealed((r) => !r)}
          >
            {revealed ? 'Hide values' : 'Reveal values'}
          </Button>
        </>
      }
    >
      <Table aria-label="Effective environment" className="table-fixed">
        <colgroup>
          <col className="w-[40%] sm:w-[30%]" />
          <col />
          <col className="hidden w-[170px] sm:table-column" />
        </colgroup>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>Key</TableHead>
            <TableHead>Value</TableHead>
            <TableHead className="hidden sm:table-cell">Source</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {loading ? (
            <TableSkeletonRows columns={3} rows={3} />
          ) : error ? (
            <TableErrorRow colSpan={3} error={error} />
          ) : merged.length === 0 ? (
            <TableMessageRow colSpan={3}>
              No variables. Add some above or link an environment group.
              {!showInjected && ` Ferry still injects ${injectedCount} variables such as ${injectedExamples}.`}
            </TableMessageRow>
          ) : (
            merged.map((item) => (
              <TableRow key={item.key}>
                <TableCell className="align-top whitespace-normal">
                  <div className="flex flex-col items-start gap-1.5">
                    <code className="font-mono text-[12.5px] break-all text-foreground">{item.key}</code>
                    {/* phones: the source column is folded under the key */}
                    <div className="flex flex-col items-start gap-1 sm:hidden">
                      <SourcePill layer={item.source} />
                      <OverrideNote item={item} />
                    </div>
                  </div>
                </TableCell>
                <TableCell className="align-top whitespace-normal">
                  <ValueCell item={item} revealed={revealed} known={known} />
                </TableCell>
                <TableCell className="hidden align-top whitespace-normal sm:table-cell">
                  <div className="flex flex-col items-start gap-1">
                    <SourcePill layer={item.source} />
                    <OverrideNote item={item} />
                  </div>
                </TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
      {!showInjected && !loading && !error && merged.length > 0 && (
        <p className="text-[12.5px] text-foreground-lighter">
          Ferry also injects {injectedCount} variables ({injectedExamples}, …); yours override them.
        </p>
      )}
    </PageSection>
  )
}

import * as React from 'react'
import { Activity, Boxes, Clock, Cpu, GitBranch, Globe, Plus, Rocket, Search, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import {
  Callout,
  ConfirmDialog,
  CopyField,
  DeployStatusBadge,
  EmptyState,
  ErrorState,
  FormActions,
  FormCard,
  FormRow,
  InfoTile,
  JobStatusBadge,
  KeyValueEditor,
  LegendDot,
  LogViewer,
  MetricCard,
  PageContainer,
  PageHeader,
  PageSection,
  SecretField,
  ServiceStateLine,
  ServiceStatePill,
  TableSkeletonRows,
  UsageBar,
  useKeyValueRows,
} from '@/components/patterns'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ApiError } from '@/lib/api/client'
import { streamPaths } from '@/lib/api/endpoints'
import { useDeploys, useServices } from '@/lib/api/queries'
import { SERVICE_STATES } from '@/lib/api/types'
import { useLogStream } from '@/lib/api/useLogStream'
import { relativeTime, shortId } from '@/lib/format'

/** Dev-only kitchen sink (`/dev/patterns`): every shared pattern with live demo data. */
export function PatternsPage() {
  const { data: services } = useServices()
  const deploys = useDeploys('api', 5)
  const kv = useKeyValueRows([
    { key: 'DATABASE_URL', value: '${{datastore.app-db.connectionString}}' },
    { key: 'LOG_LEVEL', value: 'info' },
  ])
  const [confirm, setConfirm] = React.useState(false)
  const [name, setName] = React.useState('api')
  const [source, setSource] = React.useState<'deploy' | 'runtime'>('deploy')
  const latest = deploys.data?.[0]?.id
  const path = source === 'deploy' ? (latest ? streamPaths.deployLogs(latest) : null) : streamPaths.serviceLogs('web')
  const logs = useLogStream(path, source === 'runtime' ? { tail: 200 } : {})

  return (
    <PageContainer>
      <PageHeader
        title="Patterns"
        badges={<Badge variant="warning">Dev only</Badge>}
        description="Shared building blocks rendered with live data from the connected server."
        actions={
          <>
            <Button icon={<Search />}>Default</Button>
            <Button variant="outline">Outline</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="warning">Warning</Button>
            <Button variant="danger" icon={<Trash2 />}>
              Danger
            </Button>
            <Button variant="primary" icon={<Plus />}>
              Primary
            </Button>
          </>
        }
      />

      <PageSection title="Info tiles" description="Project-home tiles: square icon + mono label + value.">
        <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
          <InfoTile icon={<Activity />} label="Status" value={<ServiceStatePill state="live" />} />
          <InfoTile icon={<Boxes />} label="Instances" value="2 running" hint="of 2 desired" />
          <InfoTile icon={<GitBranch />} label="Source" value="api-src@main" />
          <InfoTile icon={<Globe />} label="URL" value="api.localhost:19801" />
          <InfoTile icon={<Clock />} label="Latest deploy" loading value="" />
          <InfoTile icon={<Rocket />} label="Runtime" value={<Badge font="mono">node</Badge>} />
        </div>
      </PageSection>

      <PageSection title="Metric cards">
        <div className="grid gap-4 md:grid-cols-3">
          <MetricCard label="CPU" value="12.4%" info="Sum over running instances" aside={<LegendDot color="var(--brand)">cpu</LegendDot>}>
            <UsageBar value={12.4} label="CPU usage" />
          </MetricCard>
          <MetricCard label="Memory" value="182 MiB" unit="/ 512 MiB">
            <UsageBar value={78} label="Memory usage" />
          </MetricCard>
          <MetricCard label="Restarts" loading />
        </div>
      </PageSection>

      <PageSection title="Service states">
        <div className="flex flex-wrap gap-2">
          {SERVICE_STATES.map((s) => (
            <ServiceStatePill key={s} state={s} />
          ))}
          <DeployStatusBadge status="building" />
          <DeployStatusBadge status="build_failed" />
          <JobStatusBadge status="succeeded" />
          <Badge font="mono" shape="square">
            nano
          </Badge>
          <Badge variant="success">Success</Badge>
          <Badge variant="info">Info</Badge>
        </div>
        <div className="flex flex-wrap gap-6">
          {SERVICE_STATES.map((s) => (
            <ServiceStateLine key={s} state={s} />
          ))}
        </div>
      </PageSection>

      <PageSection title="General settings" description="FormCard + FormRow (form-item-layout).">
        <FormCard
          onSubmit={(e) => {
            e.preventDefault()
            toast.success('Saved', { description: `Service name: ${name}` })
          }}
          footer={<FormActions dirty={name !== 'api'} onReset={() => setName('api')} />}
        >
          <FormRow label="Service name" description="Used for the URL and the private hostname." htmlFor="demo-name">
            <Input id="demo-name" value={name} onChange={(e) => setName(e.target.value)} />
          </FormRow>
          <FormRow label="Service ID" description="Stable identifier used by the API.">
            <CopyField value="srv-dac74be26e5742dc91cc" aria-label="Service ID" />
          </FormRow>
          <FormRow label="Deploy hook" description="Secret URL that triggers a deploy.">
            <SecretField value="http://127.0.0.1:19800/hooks/deploy/srv-dac74be26e5742dc91cc?key=secret" aria-label="Deploy hook" />
          </FormRow>
          <FormRow label="Runtime" htmlFor="demo-runtime">
            <Select defaultValue="node">
              <SelectTrigger id="demo-runtime" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">Auto-detect</SelectItem>
                <SelectItem value="node">Node.js</SelectItem>
                <SelectItem value="docker">Docker</SelectItem>
              </SelectContent>
            </Select>
          </FormRow>
          <FormRow label="Auto-deploy" description="Deploy on every push to the branch.">
            <Switch defaultChecked aria-label="Auto-deploy" />
          </FormRow>
        </FormCard>
      </PageSection>

      <PageSection title="Environment variables">
        <FormCard footer={<FormActions dirty={kv.dirty} onReset={() => kv.reset([])} />} onSubmit={(e) => e.preventDefault()}>
          <FormRow label="Variables" layout="vertical">
            <KeyValueEditor rows={kv.rows} onChange={kv.setRows} errors={kv.errors} />
          </FormRow>
        </FormCard>
      </PageSection>

      <PageSection title="Table" actions={<Button size="tiny">Action</Button>}>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Deploy</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {deploys.isLoading ? (
              <TableSkeletonRows columns={3} rows={3} />
            ) : (
              deploys.data?.map((d) => (
                <TableRow key={d.id}>
                  <TableCell className="font-mono text-[13px]">{shortId(d.id)}</TableCell>
                  <TableCell>
                    <DeployStatusBadge status={d.status} />
                  </TableCell>
                  <TableCell className="text-foreground-light">{relativeTime(d.created_at)}</TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </PageSection>

      <PageSection title="Logs">
        <Tabs value={source} onValueChange={(v) => setSource(v === 'runtime' ? 'runtime' : 'deploy')}>
          <TabsList variant="pills">
            <TabsTrigger value="deploy">Build log (api)</TabsTrigger>
            <TabsTrigger value="runtime">Runtime (web)</TabsTrigger>
          </TabsList>
          <TabsContent value={source}>
            <LogViewer
              lines={logs.lines}
              status={logs.status}
              error={logs.error}
              onClear={logs.clear}
              onRestart={logs.restart}
              downloadName={source === 'deploy' ? `deploy-${latest ?? ''}` : 'web'}
              height="360px"
            />
          </TabsContent>
        </Tabs>
      </PageSection>

      <PageSection title="Empty, error and callouts">
        <EmptyState
          icon={<Boxes />}
          title="No services yet"
          description="Deploy from a git repository, a Docker image, or a local folder."
          actions={<Button variant="primary" icon={<Plus />}>New service</Button>}
        />
        <ErrorState error={new ApiError(409, 'conflict', "service 'api' is referenced by 'web'")} onRetry={() => undefined} />
        <Callout tone="warning" icon={<Cpu />} title="Degraded">
          1 of 2 instances is running.
        </Callout>
        <div>
          <Button variant="danger" icon={<Trash2 />} onClick={() => setConfirm(true)}>
            Delete service…
          </Button>
        </div>
        <ConfirmDialog
          open={confirm}
          onOpenChange={setConfirm}
          title="Delete service qa-demo"
          description="This permanently removes the service, its deploys and containers."
          confirmText="qa-demo"
          confirmLabel="Delete service"
          onConfirm={() => new Promise((resolve) => setTimeout(resolve, 600))}
        />
        {services && <p className="text-[13px] text-foreground-lighter">{services.length} services on the server.</p>}
      </PageSection>
    </PageContainer>
  )
}

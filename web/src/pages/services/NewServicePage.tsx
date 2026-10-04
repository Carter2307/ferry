import * as React from 'react'
import { Link, useLocation, useNavigate } from 'react-router'
import { AlertTriangle, ArrowLeft, Box, ChevronDown, GitBranch, Rocket, Upload } from 'lucide-react'
import { toast } from 'sonner'

import { BranchSelect } from '@/components/git/BranchSelect'
import { PushDeliveryHint } from '@/components/git/PushDeliveryHint'
import { CodeBlock } from '@/components/patterns/Copy'
import { Callout } from '@/components/patterns/EmptyState'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { ServiceTypeIcon } from '@/components/patterns/icons'
import { KeyValueEditor } from '@/components/patterns/KeyValueEditor'
import { LimitControl } from '@/components/patterns/LimitControl'
import { useKeyValueRows } from '@/components/patterns/kv-rows'
import { PageContainer, PageHeader, PageSection } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { errorMessage } from '@/lib/api/client'
import { useCreateService, useDatastores, useGitConnections, useServerInfo, useServices } from '@/lib/api/queries'
import { RUNTIMES, type GitRepository, type Runtime, type ServiceType } from '@/lib/api/types'
import { RUNTIME_LABELS, SERVICE_TYPE_LABELS } from '@/lib/format'
import { pushDelivery, serviceNameFromRepository } from '@/lib/git'
import { connectedFromState } from '@/lib/gitAuthorize'
import { LIMIT_DEFAULT, limitValue, memoryHostWarning, type LimitField } from '@/lib/resources'
import { cn } from '@/lib/utils'
import { servicePath } from '@/pages/service/context'

import { previewServiceUrl } from './lib'
import { ChoiceCards, type Choice } from './new/ChoiceCards'
import { CronField } from './new/CronField'
import { DatastoreRefMenu } from './new/DatastoreRefMenu'
import { DomainsInput } from './new/DomainsInput'
import { EnvGroupsPicker } from './new/EnvGroupsPicker'
import {
  clearDraft,
  FIELD_ORDER,
  fieldForServerError,
  INITIAL_FORM,
  MAX_INSTANCES,
  readDraft,
  saveDraft,
  toCreateRequest,
  validateForm,
  validateRepoUrl,
  visibleFields,
  type FieldKey,
  type NewServiceForm,
  type PickedRepository,
  type SourceMode,
} from './new/form'
import { RepositoryPicker } from './new/RepositoryPicker'

const TYPE_CHOICES: Choice<ServiceType>[] = [
  {
    value: 'web_service',
    label: 'Web service',
    description: 'Public HTTP app with its own URL on the proxy.',
    icon: <ServiceTypeIcon type="web_service" />,
  },
  {
    value: 'private_service',
    label: 'Private service',
    description: 'Listens on a port, reachable only from other services.',
    icon: <ServiceTypeIcon type="private_service" />,
  },
  {
    value: 'background_worker',
    label: 'Background worker',
    description: 'Long-running process without a port: queues, bots.',
    icon: <ServiceTypeIcon type="background_worker" />,
  },
  {
    value: 'cron_job',
    label: 'Cron job',
    description: 'Runs a command on a schedule, then exits.',
    icon: <ServiceTypeIcon type="cron_job" />,
  },
  {
    value: 'static_site',
    label: 'Static site',
    description: 'Built once, then served as files by the proxy.',
    icon: <ServiceTypeIcon type="static_site" />,
  },
]

const SOURCE_CHOICES: Choice<SourceMode>[] = [
  { value: 'git', label: 'Git repository', description: 'Build from a branch; redeploy on push.', icon: <GitBranch /> },
  { value: 'image', label: 'Docker image', description: 'Run a prebuilt image from a registry.', icon: <Box /> },
  { value: 'upload', label: 'Upload later', description: 'Create it now, push code with ferry up.', icon: <Upload /> },
]

const BUILD_RUNTIMES = RUNTIMES.filter((r): r is Exclude<Runtime, 'image'> => r !== 'image')

const RUNTIME_HINTS: Record<Exclude<Runtime, 'image'>, string> = {
  auto: 'Ferry uses your Dockerfile if there is one, else detects Node, Python, Go, Rust, Ruby or static files.',
  docker: 'Builds the Dockerfile of the root directory.',
  node: 'Generated Node.js image (npm, yarn or pnpm from the lockfile).',
  python: 'Generated Python image (requirements.txt or pyproject.toml).',
  go: 'Generated Go image (go.mod).',
  rust: 'Generated Rust image (Cargo.toml).',
  ruby: 'Generated Ruby image (Gemfile).',
  static: 'Serves the files as they are, no build server.',
}

/** Fields shown in the collapsible "Advanced" card. */
const ADVANCED_FIELDS: FieldKey[] = ['rootDir', 'dockerfilePath', 'healthCheckPath', 'diskMountPath', 'memoryLimit', 'cpuLimit']

const fieldId = (k: FieldKey) => `field-${k}`
const errorId = (k: FieldKey) => `field-${k}-error`

/** Focus (and scroll to) the control of a field. */
function focusField(k: FieldKey) {
  requestAnimationFrame(() => {
    // Limit controls: the custom size input when shown, else the select.
    const root = document.getElementById(`${fieldId(k)}-custom`) ?? document.getElementById(fieldId(k))
    if (!root) return
    const target =
      root.matches('input,textarea,button,select')
        ? root
        : root.querySelector<HTMLElement>('[data-state=checked], input, textarea, button')
    root.scrollIntoView({ block: 'center', behavior: 'smooth' })
    ;(target ?? root).focus({ preventScroll: true })
  })
}

/** `/services/new` — Studio settings-style form cards, "Create and deploy". */
export function NewServicePage() {
  const navigate = useNavigate()
  const location = useLocation()
  const create = useCreateService()
  const info = useServerInfo()
  const accounts = useGitConnections()
  const services = useServices()
  const datastores = useDatastores()
  const env = useKeyValueRows([])

  // Back from GitHub / GitLab after connecting an account: the form is as it was left.
  const justConnected = connectedFromState(location.state)
  const [form, setForm] = React.useState<NewServiceForm>(() => (justConnected ? readDraft() : null) ?? INITIAL_FORM)
  React.useEffect(() => saveDraft(form), [form])
  const [touched, setTouched] = React.useState<ReadonlySet<FieldKey>>(() => new Set())
  const [submitted, setSubmitted] = React.useState(false)
  const [serverError, setServerError] = React.useState<{ field: FieldKey | null; message: string } | null>(null)
  const [advancedOpen, setAdvancedOpen] = React.useState(false)
  const serverAlertRef = React.useRef<HTMLDivElement | null>(null)
  /** The name last filled in from a picked repository (replaced by the next pick, never a typed one). */
  const suggestedName = React.useRef('')

  const taken = React.useMemo(
    () => ({
      services: services.data?.map((s) => s.name) ?? [],
      datastores: datastores.data?.map((d) => d.name) ?? [],
    }),
    [services.data, datastores.data],
  )
  const hostCpus = info.data?.docker_cpus
  const errors = React.useMemo(() => validateForm(form, taken, hostCpus), [form, taken, hostCpus])
  const vis = visibleFields(form)

  const update = <K extends keyof NewServiceForm>(key: K, value: NewServiceForm[K]) => {
    setForm((f) => ({ ...f, [key]: value }))
    setServerError((e) => (e && (e.field === key || e.field === null) ? null : e))
  }
  const touch = (k: FieldKey) => setTouched((t) => (t.has(k) ? t : new Set(t).add(k)))
  /**
   * A repository was picked from a connected account (or the pick was
   * cleared): deploy its default branch, and name the service after it
   * unless a name was typed.
   */
  const pickRepository = (picked: PickedRepository | null, repository?: GitRepository) => {
    let name = form.name
    if (repository) {
      const suggestion = serviceNameFromRepository(repository.name)
      if (suggestion && (form.name.trim() === '' || form.name === suggestedName.current)) {
        name = suggestion
        suggestedName.current = suggestion
      }
    }
    setForm((f) => ({
      ...f,
      repository: picked,
      name,
      branch: repository ? (repository.default_branch ?? 'main') : f.branch,
    }))
    setServerError((e) => (e && (e.field === 'repository' || e.field === null) ? null : e))
  }
  const errorFor = (k: FieldKey): string | undefined => {
    if (serverError && serverError.field === k) return serverError.message
    const e = errors[k]
    return e && (submitted || touched.has(k)) ? e : undefined
  }
  /** Props wiring a text input to a field (id, value, blur, aria). */
  const bind = (
    k: Exclude<
      keyof NewServiceForm,
      | 'domains'
      | 'envGroups'
      | 'autoDeploy'
      | 'type'
      | 'source'
      | 'repoMode'
      | 'repository'
      | 'runtime'
      | 'memoryLimit'
      | 'cpuLimit'
    >,
  ) => {
    const err = errorFor(k)
    return {
      id: fieldId(k),
      value: form[k],
      onChange: (e: React.ChangeEvent<HTMLInputElement>) => update(k, e.target.value),
      onBlur: () => touch(k),
      'aria-invalid': err ? true : undefined,
      'aria-describedby': err ? errorId(k) : undefined,
      autoComplete: 'off',
      spellCheck: false,
    }
  }
  /** FormRow error node with a stable id (for aria-describedby). */
  const errNode = (k: FieldKey) => {
    const err = errorFor(k)
    return err ? <span id={errorId(k)}>{err}</span> : undefined
  }
  /** Props wiring a LimitControl to a field; the error shows once a custom size is typed (or on submit). */
  const bindLimit = (k: 'memoryLimit' | 'cpuLimit') => {
    const err = errorFor(k)
    return {
      id: fieldId(k),
      value: form[k],
      onChange: (v: LimitField) => {
        if (v.custom !== form[k].custom) touch(k)
        update(k, v)
      },
      info: info.data,
      invalid: Boolean(err),
      describedBy: err ? errorId(k) : undefined,
    }
  }

  const hasAdvancedValues =
    form.rootDir.trim() !== '' ||
    form.dockerfilePath.trim() !== '' ||
    form.healthCheckPath.trim() !== '' ||
    form.diskMountPath.trim() !== '' ||
    form.memoryLimit.choice !== LIMIT_DEFAULT ||
    form.cpuLimit.choice !== LIMIT_DEFAULT
  const advancedVisible = vis.rootDir || vis.dockerfilePath || vis.healthCheckPath || vis.disk || vis.limits
  const advancedParts = [
    vis.rootDir || vis.dockerfilePath ? 'monorepo paths' : null,
    vis.healthCheckPath ? 'health check' : null,
    vis.disk ? 'persistent disk' : null,
    vis.limits ? 'resource limits' : null,
  ].filter((p): p is string => p !== null)
  const advancedSummary = advancedParts.length
    ? `${advancedParts.join(', ').replace(/, ([^,]*)$/, ' and $1').replace(/^./, (c) => c.toUpperCase())}.`
    : undefined

  const perContainer = form.type === 'cron_job' ? 'each run' : 'each instance'
  const memoryWarning = errors.memoryLimit ? null : memoryHostWarning(limitValue('memory', form.memoryLimit), info.data)

  const name = form.name.trim()
  const publicUrl = vis.domains ? previewServiceUrl(name || 'my-service', info.data) : null
  const deploys = form.source !== 'upload'

  const onSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault()
    if (create.isPending) return
    setSubmitted(true)
    const first = FIELD_ORDER.find((k) => (k === 'env' ? !env.valid : Boolean(errors[k])))
    if (first) {
      if (ADVANCED_FIELDS.includes(first)) setAdvancedOpen(true)
      focusField(first)
      return
    }
    setServerError(null)
    create.mutate(toCreateRequest(form, env.vars), {
      onSuccess: (view) => {
        clearDraft()
        const deploy = view.latest_deploy
        toast.success(`${view.name} created`, {
          description: deploy ? 'The first deploy is on its way.' : `Upload your code with: ferry up ${view.name}`,
        })
        void navigate(deploy ? servicePath(view.name, `/deploys/${encodeURIComponent(deploy.id)}`) : servicePath(view.name))
      },
      onError: (err) => {
        const message = errorMessage(err)
        const field = fieldForServerError(message)
        const visibleField = field && document.getElementById(fieldId(field)) ? field : null
        setServerError({ field: visibleField, message })
        if (visibleField) {
          if (ADVANCED_FIELDS.includes(visibleField)) setAdvancedOpen(true)
          focusField(visibleField)
        } else {
          requestAnimationFrame(() => serverAlertRef.current?.focus())
        }
      },
    })
  }

  const typeLabel = SERVICE_TYPE_LABELS[form.type]
  const picked = form.repoMode === 'account' ? form.repository : null
  // The repository the branch list is read from: the picked one, or a URL
  // once it looks like one.
  const branchesOf = picked ? picked.cloneUrl : validateRepoUrl(form.repoUrl) === null ? form.repoUrl.trim() : ''
  const summary = [
    typeLabel,
    form.source === 'git' ? (picked?.fullName ?? 'Git') : form.source === 'image' ? 'Docker image' : 'Upload via CLI',
    vis.instances && form.instances.trim() ? `×${form.instances.trim()}` : null,
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <PageContainer size="narrow">
      <PageHeader
        eyebrow={
          <Link
            to="/services"
            className="inline-flex items-center gap-1.5 rounded-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          >
            <ArrowLeft className="size-3.5" aria-hidden="true" /> Services
          </Link>
        }
        title="Create a new service"
        description="Pick what to run and where the code comes from. Ferry builds it and deploys it right away."
      />

      <form noValidate onSubmit={onSubmit} aria-label="New service">
        <PageSection title="Service type">
          <ChoiceCards
            id={fieldId('type')}
            label="Service type"
            value={form.type}
            onChange={(t) => update('type', t)}
            choices={TYPE_CHOICES}
            className="sm:grid-cols-2 lg:grid-cols-3"
          />
        </PageSection>

        <PageSection title="General">
          <FormCard asDiv>
            <FormRow
              label="Name"
              htmlFor={fieldId('name')}
              description="Lowercase letters, digits and '-', starting with a letter. Also the private hostname."
              error={errNode('name')}
            >
              <Input
                {...bind('name')}
                mono
                maxLength={40}
                placeholder="my-service"
                autoFocus
                onChange={(e) => update('name', e.target.value.toLowerCase().replace(/[\s_]+/g, '-'))}
              />
              <p className="text-[12.5px] text-foreground-lighter">
                {publicUrl ? (
                  <>
                    Public URL <span className="font-mono break-all text-foreground-light">{publicUrl}</span>
                  </>
                ) : (
                  <>
                    Private hostname <span className="font-mono break-all text-foreground-light">{name || 'my-service'}</span>
                  </>
                )}
              </p>
            </FormRow>
          </FormCard>
        </PageSection>

        <PageSection title="Source" description="Where Ferry gets the code or image to run.">
          <FormCard asDiv>
            <div className="px-5 py-5 md:px-6">
              <ChoiceCards
                id={fieldId('source')}
                label="Source"
                size="sm"
                value={form.source}
                onChange={(s) => update('source', s)}
                choices={SOURCE_CHOICES}
                className="sm:grid-cols-3"
              />
            </div>
            {vis.repo && (
              <>
                <FormRow
                  label="Repository"
                  htmlFor={fieldId(form.repoMode === 'account' ? 'repository' : 'repoUrl')}
                  description="Pick it from a connected GitHub or GitLab account, or give the URL of any git repository."
                  layout="vertical"
                  error={errNode(form.repoMode === 'account' ? 'repository' : 'repoUrl')}
                >
                  <Tabs
                    value={form.repoMode}
                    onValueChange={(m) => update('repoMode', m === 'url' ? 'url' : 'account')}
                    className="gap-3"
                  >
                    <TabsList variant="pills" aria-label="How to choose the repository">
                      <TabsTrigger value="account">Connected account</TabsTrigger>
                      <TabsTrigger value="url">Repository URL</TabsTrigger>
                    </TabsList>
                    <TabsContent value="account">
                      <RepositoryPicker
                        id={fieldId('repository')}
                        value={form.repository}
                        account={justConnected}
                        onChange={pickRepository}
                        invalid={Boolean(errorFor('repository'))}
                        describedBy={errorFor('repository') ? errorId('repository') : undefined}
                      />
                    </TabsContent>
                    <TabsContent value="url" className="flex flex-col gap-1.5">
                      <Input {...bind('repoUrl')} mono placeholder="https://github.com/you/app.git" />
                      <p className="text-[12.5px] text-foreground-lighter">
                        HTTPS or SSH URL, or the absolute path of a git repository on the server. A private repository
                        on GitHub or GitLab is cloned with the account connected to this server; elsewhere it needs an
                        SSH key on the server.
                      </p>
                    </TabsContent>
                  </Tabs>
                </FormRow>
                <FormRow
                  label="Branch"
                  htmlFor={fieldId('branch')}
                  description={
                    picked
                      ? 'Deployed branch, from the repository’s own branches. Starts on its default branch.'
                      : 'Deployed branch, from the repository’s own branches. Defaults to main.'
                  }
                  error={errNode('branch')}
                >
                  <BranchSelect
                    id={fieldId('branch')}
                    repoUrl={branchesOf}
                    value={form.branch}
                    onChange={(b) => update('branch', b)}
                    onBlur={() => touch('branch')}
                    invalid={Boolean(errorFor('branch'))}
                    describedBy={errorFor('branch') ? errorId('branch') : undefined}
                  />
                </FormRow>
                <FormRow
                  label="Auto-deploy"
                  htmlFor={fieldId('autoDeploy')}
                  description={
                    <>
                      Redeploy on every push to the branch.{' '}
                      <PushDeliveryHint
                        delivery={pushDelivery(
                          accounts.data?.find((c) => c.id === picked?.connectionId),
                          accounts.data,
                          info.data,
                        )}
                      />
                    </>
                  }
                >
                  <Switch
                    id={fieldId('autoDeploy')}
                    checked={form.autoDeploy}
                    onCheckedChange={(c) => update('autoDeploy', c)}
                    aria-label="Auto-deploy"
                  />
                </FormRow>
              </>
            )}
            {vis.image && (
              <FormRow
                label="Image"
                htmlFor={fieldId('image')}
                description="Pulled on every deploy and pinned, so rollbacks are exact."
                error={errNode('image')}
              >
                <Input {...bind('image')} mono placeholder="ghcr.io/you/app:latest" />
              </FormRow>
            )}
            {form.source === 'upload' && (
              <FormRow
                label="Upload with the CLI"
                description="Run this from your project directory once the service exists. It uploads the directory and deploys it."
              >
                <CodeBlock code={`ferry up ${name || '<name>'}`} prompt />
              </FormRow>
            )}
          </FormCard>
        </PageSection>

        <PageSection title="Build & deploy">
          <FormCard asDiv>
            {vis.runtime && (
              <FormRow label="Runtime" htmlFor={fieldId('runtime')} description={RUNTIME_HINTS[form.runtime]}>
                <Select value={form.runtime} onValueChange={(v) => update('runtime', BUILD_RUNTIMES.find((r) => r === v) ?? 'auto')}>
                  <SelectTrigger id={fieldId('runtime')} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {BUILD_RUNTIMES.map((r) => (
                      <SelectItem key={r} value={r}>
                        {RUNTIME_LABELS[r]}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FormRow>
            )}
            {vis.buildCommand && (
              <FormRow
                label="Build command"
                htmlFor={fieldId('buildCommand')}
                description="Runs during the build. Leave empty for the detected default."
                error={errNode('buildCommand')}
              >
                <Input {...bind('buildCommand')} mono placeholder={form.type === 'static_site' ? 'npm ci && npm run build' : 'npm ci'} />
              </FormRow>
            )}
            {vis.publishDir && (
              <FormRow
                label="Publish directory"
                htmlFor={fieldId('publishDir')}
                description="Directory of built files to serve, relative to the root directory."
                error={errNode('publishDir')}
              >
                <Input {...bind('publishDir')} mono placeholder="dist" />
              </FormRow>
            )}
            {vis.startCommand && (
              <FormRow
                label={form.type === 'cron_job' ? 'Command' : 'Start command'}
                htmlFor={fieldId('startCommand')}
                description={
                  form.type === 'cron_job'
                    ? 'What each run executes (with sh -c).'
                    : form.source === 'image'
                      ? "Overrides the image's command (run with sh -c). Leave empty to keep it."
                      : 'Starts your app (run with sh -c). Leave empty for the detected default.'
                }
                error={errNode('startCommand')}
              >
                <Input {...bind('startCommand')} mono placeholder={form.type === 'cron_job' ? 'node scripts/report.js' : 'npm start'} />
              </FormRow>
            )}
            {vis.schedule && (
              <FormRow
                label="Schedule"
                htmlFor={fieldId('schedule')}
                description={
                  <>
                    Standard 5-field cron expression, evaluated in UTC: minute, hour, day of month, month, day of week.
                    Aliases such as <code className="font-mono">@hourly</code> and <code className="font-mono">@daily</code> work too.
                  </>
                }
                error={errNode('schedule')}
              >
                <CronField
                  id={fieldId('schedule')}
                  value={form.schedule}
                  onChange={(v) => update('schedule', v)}
                  onBlur={() => touch('schedule')}
                  invalid={Boolean(errorFor('schedule'))}
                  describedBy={errorFor('schedule') ? errorId('schedule') : undefined}
                />
              </FormRow>
            )}
            {vis.port && (
              <FormRow
                label="Port"
                htmlFor={fieldId('port')}
                description="Port your app listens on. Leave empty to detect it (PORT variable, image EXPOSE, else 10000)."
                error={errNode('port')}
              >
                <Input {...bind('port')} mono inputMode="numeric" placeholder="Auto-detect" className="sm:max-w-40" />
              </FormRow>
            )}
            {vis.instances && (
              <FormRow
                label="Instances"
                htmlFor={fieldId('instances')}
                description={`Containers running this service (1–${MAX_INSTANCES}). Web traffic is balanced across them.`}
                error={errNode('instances')}
              >
                <Input
                  {...bind('instances')}
                  mono
                  type="number"
                  min={1}
                  max={MAX_INSTANCES}
                  inputMode="numeric"
                  className="sm:max-w-40"
                />
              </FormRow>
            )}
          </FormCard>
        </PageSection>

        {advancedVisible && (
          <PageSection
            title="Advanced"
            description={advancedSummary}
            actions={
              <Button
                size="tiny"
                variant="outline"
                aria-expanded={advancedOpen}
                aria-controls="advanced-settings"
                iconRight={<ChevronDown className={cn('size-3.5 transition-transform', advancedOpen && 'rotate-180')} />}
                onClick={() => setAdvancedOpen((o) => !o)}
              >
                {advancedOpen ? 'Hide' : hasAdvancedValues ? 'Show (edited)' : 'Show'}
              </Button>
            }
          >
            <div id="advanced-settings" hidden={!advancedOpen}>
              <FormCard asDiv>
                {vis.rootDir && (
                  <FormRow
                    label="Root directory"
                    htmlFor={fieldId('rootDir')}
                    description="Subdirectory to build from, for monorepos."
                    error={errNode('rootDir')}
                  >
                    <Input {...bind('rootDir')} mono placeholder="apps/api" />
                  </FormRow>
                )}
                {vis.dockerfilePath && (
                  <FormRow
                    label="Dockerfile path"
                    htmlFor={fieldId('dockerfilePath')}
                    description="Relative to the root directory. Defaults to Dockerfile."
                    error={errNode('dockerfilePath')}
                  >
                    <Input {...bind('dockerfilePath')} mono placeholder="Dockerfile" />
                  </FormRow>
                )}
                {vis.healthCheckPath && (
                  <FormRow
                    label="Health check path"
                    htmlFor={fieldId('healthCheckPath')}
                    description="Must answer below 400 before traffic switches to a new deploy. Empty: a TCP check."
                    error={errNode('healthCheckPath')}
                  >
                    <Input {...bind('healthCheckPath')} mono placeholder="/healthz" />
                  </FormRow>
                )}
                {vis.disk && (
                  <FormRow
                    label="Disk mount path"
                    htmlFor={fieldId('diskMountPath')}
                    description="Mounts a persistent volume there. Limits the service to 1 instance; deploys restart it in place."
                    error={errNode('diskMountPath')}
                  >
                    <Input {...bind('diskMountPath')} mono placeholder="/data" />
                  </FormRow>
                )}
                {vis.limits && (
                  <>
                    <FormRow
                      label="Memory limit"
                      htmlFor={fieldId('memoryLimit')}
                      description={`Most RAM ${perContainer} may use. Past it, the process is killed (out of memory) and restarted.`}
                      error={errNode('memoryLimit')}
                    >
                      <LimitControl kind="memory" {...bindLimit('memoryLimit')} />
                      {memoryWarning && <p className="text-[13px] text-warning">{memoryWarning}</p>}
                    </FormRow>
                    <FormRow
                      label="CPU limit"
                      htmlFor={fieldId('cpuLimit')}
                      description={`CPU time ${perContainer} may use, in cores. Past it, the process is slowed down. Both limits can be changed later in Settings.`}
                      error={errNode('cpuLimit')}
                    >
                      <LimitControl kind="cpu" {...bindLimit('cpuLimit')} />
                    </FormRow>
                  </>
                )}
              </FormCard>
            </div>
          </PageSection>
        )}

        {vis.domains && (
          <PageSection title="Custom domains">
            <FormCard asDiv>
              <FormRow
                label="Domains"
                htmlFor={fieldId('domains')}
                description="Point each domain's DNS at this server. Press Enter to add one."
                error={errNode('domains')}
              >
                <DomainsInput
                  id={fieldId('domains')}
                  value={form.domains}
                  onChange={(d) => update('domains', d)}
                  draft={form.domainDraft}
                  onDraftChange={(d) => update('domainDraft', d)}
                  onInvalidDraft={() => touch('domains')}
                  invalid={Boolean(errorFor('domains'))}
                  describedBy={errorFor('domains') ? errorId('domains') : undefined}
                />
              </FormRow>
            </FormCard>
          </PageSection>
        )}

        <PageSection title="Environment">
          <FormCard asDiv>
            <FormRow
              label={
                <span className="flex items-center justify-between gap-3">
                  Environment variables
                  <DatastoreRefMenu rows={env.rows} onChange={env.setRows} />
                </span>
              }
              layout="vertical"
              description={
                <>
                  Available at build and run time. Datastores and services can be referenced, e.g.{' '}
                  <code className="font-mono text-[12.5px] whitespace-nowrap">{`\${{datastore.${datastores.data?.[0]?.name ?? 'my-db'}.connectionString}}`}</code>
                </>
              }
            >
              <div id={fieldId('env')}>
                <KeyValueEditor rows={env.rows} onChange={env.setRows} errors={env.errors} addLabel="Add variable" />
              </div>
            </FormRow>
            <FormRow
              label="Env groups"
              htmlFor={fieldId('envGroups')}
              description="Shared variables linked to this service. Its own variables win on conflicts."
              error={errNode('envGroups')}
            >
              <EnvGroupsPicker
                id={fieldId('envGroups')}
                value={form.envGroups}
                onChange={(g) => update('envGroups', g)}
                invalid={Boolean(errorFor('envGroups'))}
                describedBy={errorFor('envGroups') ? errorId('envGroups') : undefined}
              />
            </FormRow>
          </FormCard>
        </PageSection>

        {serverError && !serverError.field && (
          <div ref={serverAlertRef} tabIndex={-1} className="mt-8 outline-none">
            <Callout tone="destructive" icon={<AlertTriangle />} title="The service could not be created">
              {serverError.message}
            </Callout>
          </div>
        )}

        <div className="sticky bottom-4 z-10 mt-8 flex flex-wrap items-center gap-3 rounded-lg border bg-surface-100/95 px-4 py-3 shadow-overlay backdrop-blur-sm">
          <div className="hidden min-w-0 flex-1 items-center gap-2 text-[13px] text-foreground-light sm:flex">
            <ServiceTypeIcon type={form.type} className="size-4 shrink-0 text-foreground-lighter" />
            <span className="truncate">
              <span className="font-medium text-foreground">{name || 'New service'}</span>
              <span className="text-foreground-lighter"> · {summary}</span>
            </span>
          </div>
          <div className="flex flex-1 items-center justify-end gap-2 sm:flex-none">
            <Button asChild size="md">
              <Link to="/services">Cancel</Link>
            </Button>
            <Button type="submit" variant="primary" size="md" icon={deploys ? <Rocket /> : undefined} loading={create.isPending}>
              {deploys ? 'Create and deploy' : 'Create service'}
            </Button>
          </div>
        </div>
      </form>
    </PageContainer>
  )
}

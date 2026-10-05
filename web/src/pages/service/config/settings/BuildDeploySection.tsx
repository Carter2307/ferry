import * as React from 'react'
import { useNavigate } from 'react-router'
import { Info } from 'lucide-react'
import { toast } from 'sonner'

import { BranchSelect } from '@/components/git/BranchSelect'
import { PushDeliveryHint } from '@/components/git/PushDeliveryHint'
import { CodeBlock } from '@/components/patterns/Copy'
import { Callout } from '@/components/patterns/EmptyState'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageSection } from '@/components/patterns/Page'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { errorMessage } from '@/lib/api/client'
import { useGitConnections, useServerInfo, useTriggerDeploy, useUpdateService } from '@/lib/api/queries'
import type { ServiceView, SourceKind } from '@/lib/api/types'
import { RUNTIME_LABELS } from '@/lib/format'
import { connectionLabel, GIT_PROVIDER_LABELS, pushDelivery } from '@/lib/git'

import { servicePath } from '../../context'
import { useReportDirty, useSyncedForm } from '../hooks'
import {
  BUILD_RUNTIMES,
  buildFields,
  buildFormFrom,
  buildPatch,
  repoUrlError,
  sameForm,
  validateBuildForm,
  type BuildForm,
} from './forms'
import { CommandInput, FormError, SaveFooter, SourcePicker, StaleCallout } from './parts'

const SOURCE_NAMES: Record<SourceKind, string> = { git: 'the git repository', image: 'the image', upload: 'uploads' }

/** Build & deploy: source (git / image / upload), runtime, paths, commands, port, health check, auto-deploy. */
export function BuildDeploySection({
  service,
  reportDirty,
}: {
  service: ServiceView
  reportDirty: (id: string, dirty: boolean) => void
}) {
  const name = service.name
  const navigate = useNavigate()
  const update = useUpdateService(name)
  const deploy = useTriggerDeploy(name)
  const info = useServerInfo()
  const accounts = useGitConnections()
  const server = React.useMemo(() => buildFormFrom(service), [service])
  const form = useSyncedForm<BuildForm>(server, sameForm)
  const [submitted, setSubmitted] = React.useState(false)
  const [saveError, setSaveError] = React.useState<string | null>(null)
  const ids = React.useId()
  const id = (k: string) => `${ids}-${k}`

  useReportDirty(reportDirty, 'build', form.dirty)

  const f = form.value
  const show = buildFields(service.type, f)
  const errors = validateBuildForm(f, service.type)
  const invalid = Object.keys(errors).length > 0
  const err = (k: keyof BuildForm) => (submitted || f[k] !== form.base[k] ? errors[k] : undefined)
  const isCron = service.type === 'cron_job'
  const switching = f.source !== form.base.source
  // The account of this server that clones the repository as it is saved
  // (accounts belong to the server: nothing to choose here).
  const cloner =
    f.repo_url.trim() === form.base.repo_url.trim()
      ? accounts.data?.find((c) => c.status === 'connected' && c.services.includes(name))
      : undefined

  const set = <K extends keyof BuildForm>(k: K) => (v: BuildForm[K]) => form.patch({ [k]: v } as Partial<BuildForm>)
  const text = (k: keyof BuildForm) => ({
    id: id(k),
    value: String(f[k]),
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => form.patch({ [k]: e.target.value } as Partial<BuildForm>),
    'aria-invalid': err(k) ? true : undefined,
  })

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    setSubmitted(true)
    setSaveError(null)
    if (invalid || !form.dirty) return
    const patch = buildPatch(form.base, f, service.type)
    if (Object.keys(patch).length === 0) {
      form.reset()
      return
    }
    update.mutate(patch, {
      onSuccess: (view) => {
        form.commit(buildFormFrom(view))
        setSubmitted(false)
        const canDeploy = !view.suspended && (view.repo_url !== null || view.image !== null || view.live_deploy_id !== null)
        toast.success('Build & deploy settings saved', {
          description: 'They apply on the next deploy.',
          action: canDeploy
            ? {
                label: 'Deploy now',
                onClick: () =>
                  deploy.mutate(
                    {},
                    {
                      onSuccess: (d) => void navigate(servicePath(name, `/deploys/${d.id}`)),
                      onError: (e2) => toast.error('Could not start the deploy', { description: errorMessage(e2) }),
                    },
                  ),
              }
            : undefined,
        })
      },
      onError: (e2) => {
        setSaveError(errorMessage(e2))
        toast.error('Could not save the settings', { description: errorMessage(e2) })
      },
    })
  }

  return (
    <PageSection
      id="build"
      title="Build & deploy"
      description={
        isCron
          ? 'Where the job’s code comes from and how it is built. Changing the command restarts the schedule with it.'
          : 'Where the code comes from, how it is built and how it starts. Changes apply on the next deploy.'
      }
    >
      <FormCard
        aria-label="Build and deploy settings"
        onSubmit={onSubmit}
        footer={
          <SaveFooter
            dirty={form.dirty}
            saving={update.isPending}
            invalid={submitted && invalid}
            onCancel={() => {
              form.reset()
              setSubmitted(false)
              setSaveError(null)
            }}
            hint="Changes apply on the next deploy"
          />
        }
      >
        {form.stale && <StaleCallout onLoad={form.loadLatest} />}
        <FormError message={saveError} />

        <FormRow
          label={<span id={id('source-label')}>Source</span>}
          description="A service deploys from one source. Switching clears the previous one when you save."
          layout="vertical"
        >
          <SourcePicker value={f.source} onChange={set('source')} labelledBy={id('source-label')} />
          {switching && form.base.source !== 'upload' && (
            <Callout tone="warning" icon={<Info />} className="mt-1">
              Saving switches {name} from {SOURCE_NAMES[form.base.source]} to {SOURCE_NAMES[f.source]} and forgets{' '}
              {form.base.source === 'git' ? `the repository ${form.base.repo_url}` : `the image ${form.base.image}`}. The
              running deploy is not affected until the next deploy.
            </Callout>
          )}
        </FormRow>

        {show.repo && (
          <>
            <FormRow
              label="Repository URL"
              htmlFor={id('repo_url')}
              description={
                <>
                  An https://, ssh:// or git@host:path URL, or an absolute path on the server.
                  {cloner && (
                    <>
                      {' '}
                      Cloned with the {GIT_PROVIDER_LABELS[cloner.provider]} account{' '}
                      <span className="text-foreground-light">{connectionLabel(cloner)}</span> connected to this server.
                    </>
                  )}
                </>
              }
              error={err('repo_url')}
            >
              <Input mono placeholder="https://github.com/acme/app.git" autoComplete="off" spellCheck={false} {...text('repo_url')} />
            </FormRow>
            <FormRow
              label="Branch"
              htmlFor={id('branch')}
              description="Deploys build the tip of this branch. The list is read from the repository."
              error={err('branch')}
            >
              <BranchSelect
                id={id('branch')}
                repoUrl={repoUrlError(f.repo_url) === null ? f.repo_url : ''}
                value={f.branch}
                onChange={set('branch')}
                invalid={Boolean(err('branch'))}
              />
            </FormRow>
          </>
        )}

        {show.image && (
          <FormRow
            label="Image"
            htmlFor={id('image')}
            description="Registry reference. Tags like latest are pulled again on every deploy; each deploy pins the exact image."
            error={err('image')}
          >
            <Input mono placeholder="ghcr.io/acme/app:1.4.2" autoComplete="off" spellCheck={false} {...text('image')} />
          </FormRow>
        )}

        {f.source === 'upload' && (
          <FormRow
            label="Upload code"
            description="Deploys use the most recent upload. Push new code from your machine with the CLI."
          >
            <CodeBlock code={`ferry up ${name}`} prompt />
          </FormRow>
        )}

        {show.runtime && (
          <FormRow
            label="Runtime"
            htmlFor={id('runtime')}
            description="How the image is built. Auto-detect picks a Dockerfile, then the language from files such as package.json or go.mod."
          >
            <Select value={f.runtime} onValueChange={(v) => set('runtime')(v as BuildForm['runtime'])}>
              <SelectTrigger id={id('runtime')} className="w-full">
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

        {show.rootDir && (
          <FormRow
            label="Root directory"
            htmlFor={id('root_dir')}
            description="Build from this folder of the repository (monorepos). Empty = the repository root."
            error={err('root_dir')}
          >
            <Input mono placeholder="apps/api" autoComplete="off" spellCheck={false} {...text('root_dir')} />
          </FormRow>
        )}

        {show.dockerfilePath && (
          <FormRow
            label="Dockerfile path"
            htmlFor={id('dockerfile_path')}
            description="Relative to the root directory. Empty = ./Dockerfile."
            error={err('dockerfile_path')}
          >
            <Input mono placeholder="Dockerfile" autoComplete="off" spellCheck={false} {...text('dockerfile_path')} />
          </FormRow>
        )}

        {show.buildCommand && (
          <FormRow
            label="Build command"
            htmlFor={id('build_command')}
            description="Runs while the image is built. Empty = the runtime’s default."
          >
            <CommandInput placeholder="npm ci && npm run build" {...text('build_command')} />
          </FormRow>
        )}

        {show.startCommand && (
          <FormRow
            label={isCron ? 'Command' : 'Start command'}
            htmlFor={id('start_command')}
            description={
              isCron
                ? 'What each scheduled run executes. Empty = the image’s default command.'
                : f.source === 'image' || f.runtime === 'docker'
                  ? 'Overrides the image’s CMD (runs with /bin/sh -c). Empty = the image default.'
                  : 'How the app starts. Empty = the runtime’s default.'
            }
          >
            <CommandInput placeholder={isCron ? 'python jobs/nightly.py' : 'node server.js'} {...text('start_command')} />
          </FormRow>
        )}

        {show.publishDir && (
          <FormRow
            label="Publish directory"
            htmlFor={id('publish_dir')}
            description="Folder with the built static files. Empty = detected (dist, build, public…)."
            error={err('publish_dir')}
          >
            <Input mono placeholder="dist" autoComplete="off" spellCheck={false} {...text('publish_dir')} />
          </FormRow>
        )}

        {show.port && (
          <FormRow
            label="Port"
            htmlFor={id('port')}
            description={
              <>
                The port the app listens on. Empty = auto: your <code className="font-mono">PORT</code> variable, then what
                the build or image exposes, then the server default.
                {service.internal_port !== null && <> Currently {service.internal_port}.</>}
              </>
            }
            error={err('port')}
          >
            <Input
              mono
              inputMode="numeric"
              placeholder="auto"
              autoComplete="off"
              className="sm:max-w-40"
              {...text('port')}
            />
          </FormRow>
        )}

        {show.healthCheck && (
          <FormRow
            label="Health check path"
            htmlFor={id('health_check_path')}
            description="A deploy goes live once this path answers below 400. Empty = a TCP connection is enough."
            error={err('health_check_path')}
          >
            <Input mono placeholder="/healthz" autoComplete="off" spellCheck={false} {...text('health_check_path')} />
          </FormRow>
        )}

        {show.autoDeploy && (
          <FormRow
            label="Auto-deploy"
            htmlFor={id('auto_deploy')}
            description={
              <>
                Deploy on every push to <code className="font-mono">{f.branch || 'the branch'}</code>.{' '}
                <PushDeliveryHint delivery={pushDelivery(cloner, accounts.data, info.data)} />
              </>
            }
          >
            <Switch id={id('auto_deploy')} checked={f.auto_deploy} onCheckedChange={set('auto_deploy')} />
          </FormRow>
        )}
      </FormCard>
    </PageSection>
  )
}

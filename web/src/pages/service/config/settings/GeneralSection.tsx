import { CopyField } from '@/components/patterns/Copy'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { ServiceTypeIcon } from '@/components/patterns/icons'
import { PageSection } from '@/components/patterns/Page'
import type { ServiceType, ServiceView } from '@/lib/api/types'
import { dateTime, relativeTime, SERVICE_TYPE_LABELS } from '@/lib/format'

const TYPE_HELP: Record<ServiceType, string> = {
  web_service: 'Public HTTP service behind the proxy, with its own URL.',
  private_service: 'Reachable only from other services on the private network.',
  background_worker: 'Long-running process without a port.',
  cron_job: 'Runs a command on a schedule.',
  static_site: 'Static files served over HTTP.',
}

/** General: name, type, id, private address, created (read-only with Copy). */
export function GeneralSection({ service }: { service: ServiceView }) {
  const address = service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host
  return (
    <PageSection id="general" title="General" description="Identity of the service. The name and type are set when it is created.">
      <FormCard asDiv aria-label="General settings">
        <FormRow label="Service name" htmlFor="svc-name" description="Used in its URL and as its hostname on the private network.">
          <CopyField id="svc-name" value={service.name} what="service name" />
        </FormRow>
        <FormRow label="Service ID" htmlFor="svc-id" description="Stable identifier for the API and deploy hooks.">
          <CopyField id="svc-id" value={service.id} what="service ID" />
        </FormRow>
        <FormRow label="Type" description={TYPE_HELP[service.type]}>
          <div className="flex h-[34px] items-center gap-2 text-sm text-foreground">
            <ServiceTypeIcon type={service.type} className="size-4 text-foreground-lighter" />
            {SERVICE_TYPE_LABELS[service.type]}
          </div>
        </FormRow>
        {service.type !== 'cron_job' && (
          <FormRow
            label="Private address"
            htmlFor="svc-address"
            description="Other services and jobs reach it at this address on the private network."
          >
            <CopyField id="svc-address" value={address} what="private address" />
          </FormRow>
        )}
        <FormRow label="Created">
          <div className="flex h-[34px] items-center gap-2 text-sm text-foreground">
            <time dateTime={service.created_at} title={service.created_at}>
              {dateTime(service.created_at)}
            </time>
            <span className="text-foreground-lighter">· {relativeTime(service.created_at)}</span>
          </div>
        </FormRow>
      </FormCard>
    </PageSection>
  )
}

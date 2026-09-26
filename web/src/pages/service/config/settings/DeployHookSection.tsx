import * as React from 'react'
import { RefreshCw } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { SecretField } from '@/components/patterns/Copy'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageSection } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import { useRotateDeployHook } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'

/** Secret deploy hook URL (reveal / copy) and key rotation. */
export function DeployHookSection({ service }: { service: ServiceView }) {
  const rotate = useRotateDeployHook(service.name)
  const [confirm, setConfirm] = React.useState(false)
  const url = `${window.location.origin}${service.deploy_hook_path}`

  return (
    <PageSection
      id="deploy-hook"
      title="Deploy hook"
      description="A secret URL that deploys the service when called — for CI pipelines or registry webhooks."
    >
      <FormCard asDiv aria-label="Deploy hook">
        <FormRow
          label="Hook URL"
          htmlFor="svc-hook"
          description={
            <>
              Send a <code className="font-mono">POST</code> (or <code className="font-mono">GET</code>) request to it. Keep it
              private: anyone with the URL can deploy.
            </>
          }
        >
          {/* key the field so a rotated URL starts hidden again */}
          <SecretField key={service.deploy_hook_key} id="svc-hook" value={url} what="deploy hook URL" size="md" />
        </FormRow>
        <FormRow label="Regenerate" description="Creates a new secret key. The current URL stops working immediately.">
          <div>
            <Button variant="warning" icon={<RefreshCw />} loading={rotate.isPending} onClick={() => setConfirm(true)}>
              Regenerate hook URL
            </Button>
          </div>
        </FormRow>
      </FormCard>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        variant="warning"
        title="Regenerate the deploy hook?"
        description={
          <p>
            The current URL of {service.name} stops working right away. Update every CI job or webhook that calls it with the
            new URL.
          </p>
        }
        confirmLabel="Regenerate"
        onConfirm={() =>
          rotate.mutateAsync().then(() => {
            toast.success('Deploy hook regenerated', { description: 'Copy the new URL into your CI.' })
          })
        }
      />
    </PageSection>
  )
}

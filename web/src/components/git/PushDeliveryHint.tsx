import { Link } from 'react-router'

import type { PushDelivery } from '@/lib/git'

const LINK =
  'rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring'

/**
 * Next to an auto-deploy switch: how the pushes to the service's repository
 * reach this server (`pushDelivery`), and what to do when nothing delivers
 * them. Nothing while that isn't known.
 */
export function PushDeliveryHint({ delivery }: { delivery: PushDelivery }) {
  if (delivery === 'app') {
    return <>GitHub tells this server about each push: there is nothing to add to the repository.</>
  }
  if (delivery === 'webhook') {
    return (
      <>
        The repository needs the server’s webhook (
        <Link to="/server?section=connections" className={LINK}>
          Connections
        </Link>
        ), unless its GitHub account deploys on push.
      </>
    )
  }
  if (delivery === 'none') {
    return (
      <>
        Pushes do not reach this server yet: connect the repository’s GitHub account from the server’s public address (
        <Link to="/server?section=connections" className={LINK}>
          Connections
        </Link>
        ), or call the deploy hook from your CI.
      </>
    )
  }
  return null
}

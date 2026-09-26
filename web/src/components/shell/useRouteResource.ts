import { useMatch } from 'react-router'

export type RouteResource =
  | { kind: 'service'; name: string }
  | { kind: 'datastore'; name: string }
  | { kind: 'env-group'; name: string }
  | { kind: null; name: null }

/** Which resource detail page is open (drives the breadcrumb switchers, ⌘K actions, Connect). */
export function useRouteResource(): RouteResource {
  const service = useMatch('/services/:name/*')
  const datastore = useMatch('/datastores/:name/*')
  const group = useMatch('/env-groups/:name/*')
  const sName = service?.params.name
  if (sName && sName !== 'new') return { kind: 'service', name: sName }
  if (datastore?.params.name) return { kind: 'datastore', name: datastore.params.name }
  if (group?.params.name) return { kind: 'env-group', name: group.params.name }
  return { kind: null, name: null }
}

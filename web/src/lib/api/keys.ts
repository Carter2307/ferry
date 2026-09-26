/**
 * TanStack Query key factory. Hierarchical so a prefix invalidates a subtree:
 * `keys.service(ref)` covers the service's status, deploys, env, domains and jobs.
 * `ref` is whatever identifies the resource in the URL (name or id).
 */
export const keys = {
  info: () => ['info'] as const,

  services: () => ['services'] as const,
  serviceList: () => ['services', 'list'] as const,
  service: (ref: string) => ['services', 'detail', ref] as const,
  serviceStatus: (ref: string) => ['services', 'detail', ref, 'status'] as const,
  serviceDeploys: (ref: string, limit?: number) =>
    limit === undefined
      ? (['services', 'detail', ref, 'deploys'] as const)
      : (['services', 'detail', ref, 'deploys', { limit }] as const),
  serviceEnv: (ref: string) => ['services', 'detail', ref, 'env'] as const,
  serviceDomains: (ref: string) => ['services', 'detail', ref, 'domains'] as const,
  serviceJobs: (ref: string, limit?: number) =>
    limit === undefined
      ? (['services', 'detail', ref, 'jobs'] as const)
      : (['services', 'detail', ref, 'jobs', { limit }] as const),

  deploys: () => ['deploys'] as const,
  deploy: (id: string) => ['deploys', id] as const,

  jobs: () => ['jobs'] as const,
  job: (id: string) => ['jobs', id] as const,

  datastores: () => ['datastores'] as const,
  datastoreList: () => ['datastores', 'list'] as const,
  datastore: (ref: string) => ['datastores', 'detail', ref] as const,

  envGroups: () => ['env-groups'] as const,
  envGroupList: () => ['env-groups', 'list'] as const,
  envGroup: (ref: string) => ['env-groups', 'detail', ref] as const,
} as const

/**
 * Raw API calls (DESIGN.md §10). One function per endpoint; the React hooks in
 * `queries/*` wrap these. `ref` = resource id or name.
 */

import { api, seg } from './client'
import type {
  ApplyBlueprint,
  BlueprintResult,
  CreateDatastore,
  CreateEnvGroup,
  CreateService,
  DatastoreView,
  Deploy,
  EnvGroupView,
  EnvVar,
  JobRun,
  PatchEnv,
  ReplaceEnv,
  RunJobRequest,
  RuntimeStatus,
  ServerInfo,
  ServiceView,
  TriggerDeploy,
  UpdateService,
} from './types'

const V1 = '/api/v1'

export const endpoints = {
  // info
  info: (signal?: AbortSignal) => api.get<ServerInfo>(`${V1}/info`, undefined, signal),

  // services
  listServices: (signal?: AbortSignal) => api.get<ServiceView[]>(`${V1}/services`, undefined, signal),
  getService: (ref: string, signal?: AbortSignal) => api.get<ServiceView>(`${V1}/services/${seg(ref)}`, undefined, signal),
  createService: (body: CreateService) => api.post<ServiceView>(`${V1}/services`, body),
  updateService: (ref: string, body: UpdateService) => api.patch<ServiceView>(`${V1}/services/${seg(ref)}`, body),
  deleteService: (ref: string, force = false) =>
    api.delete(`${V1}/services/${seg(ref)}`, force ? { force: true } : undefined),
  serviceStatus: (ref: string, signal?: AbortSignal) =>
    api.get<RuntimeStatus>(`${V1}/services/${seg(ref)}/status`, undefined, signal),
  restartService: (ref: string) => api.post<Deploy>(`${V1}/services/${seg(ref)}/restart`),
  suspendService: (ref: string) => api.post<ServiceView>(`${V1}/services/${seg(ref)}/suspend`),
  resumeService: (ref: string) => api.post<ServiceView>(`${V1}/services/${seg(ref)}/resume`),
  scaleService: (ref: string, instances: number) =>
    api.post<ServiceView>(`${V1}/services/${seg(ref)}/scale`, { instances }),
  rollbackService: (ref: string, deployId: string) =>
    api.post<Deploy>(`${V1}/services/${seg(ref)}/rollback`, { deploy_id: deployId }),
  rotateDeployHook: (ref: string) => api.post<ServiceView>(`${V1}/services/${seg(ref)}/deploy-hook/rotate`),

  // deploys
  listDeploys: (ref: string, limit = 20, signal?: AbortSignal) =>
    api.get<Deploy[]>(`${V1}/services/${seg(ref)}/deploys`, { limit }, signal),
  triggerDeploy: (ref: string, body: TriggerDeploy = {}) =>
    api.post<Deploy>(`${V1}/services/${seg(ref)}/deploys`, body),
  getDeploy: (id: string, signal?: AbortSignal) => api.get<Deploy>(`${V1}/deploys/${seg(id)}`, undefined, signal),
  cancelDeploy: (id: string) => api.post<Deploy>(`${V1}/deploys/${seg(id)}/cancel`),

  // env
  getServiceEnv: (ref: string, signal?: AbortSignal) =>
    api.get<EnvVar[]>(`${V1}/services/${seg(ref)}/env`, undefined, signal),
  replaceServiceEnv: (ref: string, body: ReplaceEnv, restart = false) =>
    api.put<EnvVar[]>(`${V1}/services/${seg(ref)}/env`, body, restart ? { restart: true } : undefined),
  patchServiceEnv: (ref: string, body: PatchEnv, restart = false) =>
    api.patch<EnvVar[]>(`${V1}/services/${seg(ref)}/env`, body, restart ? { restart: true } : undefined),
  linkEnvGroup: (ref: string, group: string) =>
    api.post<ServiceView>(`${V1}/services/${seg(ref)}/env-groups`, { group }),
  unlinkEnvGroup: (ref: string, group: string) =>
    api.delete<ServiceView>(`${V1}/services/${seg(ref)}/env-groups/${seg(group)}`),

  // domains
  listDomains: (ref: string, signal?: AbortSignal) =>
    api.get<string[]>(`${V1}/services/${seg(ref)}/domains`, undefined, signal),
  addDomain: (ref: string, domain: string) => api.post<string[]>(`${V1}/services/${seg(ref)}/domains`, { domain }),
  removeDomain: (ref: string, domain: string) =>
    api.delete<string[]>(`${V1}/services/${seg(ref)}/domains/${seg(domain)}`),

  // jobs
  listJobs: (ref: string, limit = 20, signal?: AbortSignal) =>
    api.get<JobRun[]>(`${V1}/services/${seg(ref)}/jobs`, { limit }, signal),
  runJob: (ref: string, body: RunJobRequest = {}) => api.post<JobRun>(`${V1}/services/${seg(ref)}/jobs`, body),
  getJob: (id: string, signal?: AbortSignal) => api.get<JobRun>(`${V1}/jobs/${seg(id)}`, undefined, signal),
  cancelJob: (id: string) => api.post<JobRun>(`${V1}/jobs/${seg(id)}/cancel`),

  // datastores
  listDatastores: (signal?: AbortSignal) => api.get<DatastoreView[]>(`${V1}/datastores`, undefined, signal),
  getDatastore: (ref: string, signal?: AbortSignal) =>
    api.get<DatastoreView>(`${V1}/datastores/${seg(ref)}`, undefined, signal),
  createDatastore: (body: CreateDatastore) => api.post<DatastoreView>(`${V1}/datastores`, body),
  deleteDatastore: (ref: string, force = false) =>
    api.delete(`${V1}/datastores/${seg(ref)}`, force ? { force: true } : undefined),

  // env groups
  listEnvGroups: (signal?: AbortSignal) => api.get<EnvGroupView[]>(`${V1}/env-groups`, undefined, signal),
  getEnvGroup: (ref: string, signal?: AbortSignal) =>
    api.get<EnvGroupView>(`${V1}/env-groups/${seg(ref)}`, undefined, signal),
  createEnvGroup: (body: CreateEnvGroup) => api.post<EnvGroupView>(`${V1}/env-groups`, body),
  deleteEnvGroup: (ref: string, opts: { force?: boolean; restart?: boolean } = {}) =>
    api.delete(`${V1}/env-groups/${seg(ref)}`, {
      force: opts.force ? true : undefined,
      restart: opts.restart ? true : undefined,
    }),
  replaceEnvGroupEnv: (ref: string, body: ReplaceEnv, restart = false) =>
    api.put<EnvGroupView>(`${V1}/env-groups/${seg(ref)}/env`, body, restart ? { restart: true } : undefined),
  patchEnvGroupEnv: (ref: string, body: PatchEnv, restart = false) =>
    api.patch<EnvGroupView>(`${V1}/env-groups/${seg(ref)}/env`, body, restart ? { restart: true } : undefined),

  // blueprints
  applyBlueprint: (body: ApplyBlueprint) => api.post<BlueprintResult>(`${V1}/blueprints/apply`, body),
}

/** SSE paths (consumed by useLogStream / events.ts). */
export const streamPaths = {
  events: `${V1}/events`,
  deployLogs: (deployId: string) => `${V1}/deploys/${seg(deployId)}/logs`,
  jobLogs: (jobId: string) => `${V1}/jobs/${seg(jobId)}/logs`,
  serviceLogs: (ref: string) => `${V1}/services/${seg(ref)}/logs`,
}

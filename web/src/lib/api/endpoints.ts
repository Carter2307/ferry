/**
 * Raw API calls (DESIGN.md §10). One function per endpoint; the React hooks in
 * `queries/*` wrap these. `ref` = resource id or name.
 */

import { api, request, seg } from './client'
import type {
  ApiTokenView,
  ApplyBlueprint,
  AuthorizeGit,
  AuthStatus,
  BlueprintResult,
  CertificateView,
  ChangePassword,
  CliLoginView,
  ConnectGit,
  CreateApiToken,
  CreatedApiToken,
  CreateDatastore,
  CreateEnvGroup,
  CreateService,
  DatastoreView,
  Deploy,
  DomainView,
  EnvGroupView,
  EnvVar,
  GitAuthorization,
  GitBranches,
  GitCallback,
  GitConnectionView,
  GitRepositoryList,
  JobRun,
  Login,
  PatchEnv,
  ReplaceEnv,
  RunJobRequest,
  RuntimeStatus,
  ServerInfo,
  ServiceView,
  SessionView,
  SetupAccount,
  TriggerDeploy,
  UpdateDatastore,
  UpdateService,
} from './types'

const V1 = '/api/v1'

export const endpoints = {
  // the account, its sessions and API tokens (the answers of the first three
  // say whether there is a session: a 401 of theirs ends none)
  authStatus: (signal?: AbortSignal) =>
    request<AuthStatus>(`${V1}/auth/status`, { signal, skipAuthRedirect: true }),
  setupAccount: (body: SetupAccount) =>
    request<AuthStatus>(`${V1}/auth/setup`, { method: 'POST', body, skipAuthRedirect: true }),
  login: (body: Login) => request<AuthStatus>(`${V1}/auth/login`, { method: 'POST', body, skipAuthRedirect: true }),
  logout: () => request<void>(`${V1}/auth/logout`, { method: 'POST', skipAuthRedirect: true }),
  changePassword: (body: ChangePassword) => api.post<void>(`${V1}/auth/password`, body),
  listSessions: (signal?: AbortSignal) => api.get<SessionView[]>(`${V1}/auth/sessions`, undefined, signal),
  endSession: (id: string) => api.delete(`${V1}/auth/sessions/${seg(id)}`),
  listApiTokens: (signal?: AbortSignal) => api.get<ApiTokenView[]>(`${V1}/auth/tokens`, undefined, signal),
  createApiToken: (body: CreateApiToken) => api.post<CreatedApiToken>(`${V1}/auth/tokens`, body),
  revokeApiToken: (id: string) => api.delete(`${V1}/auth/tokens/${seg(id)}`),
  getCliLogin: (id: string, signal?: AbortSignal) =>
    api.get<CliLoginView>(`${V1}/auth/cli/${seg(id)}`, undefined, signal),
  approveCliLogin: (id: string) => api.post<CliLoginView>(`${V1}/auth/cli/${seg(id)}/approve`),
  denyCliLogin: (id: string) => api.post<CliLoginView>(`${V1}/auth/cli/${seg(id)}/deny`),

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

  // custom domains of a service
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
  updateDatastore: (ref: string, body: UpdateDatastore) =>
    api.patch<DatastoreView>(`${V1}/datastores/${seg(ref)}`, body),
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

  // git connections
  listGitConnections: (signal?: AbortSignal) =>
    api.get<GitConnectionView[]>(`${V1}/git/connections`, undefined, signal),
  authorizeGit: (body: AuthorizeGit) => api.post<GitAuthorization>(`${V1}/git/authorize`, body),
  gitCallback: (body: GitCallback) => api.post<GitAuthorization>(`${V1}/git/callback`, body),
  connectGit: (body: ConnectGit) => api.post<GitConnectionView>(`${V1}/git/connections`, body),
  deleteGitConnection: (id: string, force = false) =>
    api.delete(`${V1}/git/connections/${seg(id)}`, force ? { force: true } : undefined),
  listGitRepositories: (id: string, signal?: AbortSignal) =>
    api.get<GitRepositoryList>(`${V1}/git/connections/${seg(id)}/repositories`, undefined, signal),
  listGitBranches: (repoUrl: string, signal?: AbortSignal) =>
    api.get<GitBranches>(`${V1}/git/branches`, { repo_url: repoUrl }, signal),

  // the server's domains (services are served under them) and certificates
  listServerDomains: (signal?: AbortSignal) => api.get<DomainView[]>(`${V1}/domains`, undefined, signal),
  connectDomain: (name: string) => api.post<DomainView>(`${V1}/domains`, { name }),
  verifyDomain: (ref: string) => api.post<DomainView>(`${V1}/domains/${seg(ref)}/verify`),
  setDefaultDomain: (ref: string) => api.patch<DomainView>(`${V1}/domains/${seg(ref)}`, { is_default: true }),
  disconnectDomain: (ref: string) => api.delete(`${V1}/domains/${seg(ref)}`),
  listCertificates: (signal?: AbortSignal) => api.get<CertificateView[]>(`${V1}/certificates`, undefined, signal),

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

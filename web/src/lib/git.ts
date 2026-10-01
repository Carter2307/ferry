/**
 * Git connections (the GitHub / GitLab accounts this server is authorized to
 * read): labels, the links to the providers' pages, how an account is
 * described, and repository search.
 */

import type { GitAuth, GitConnectionView, GitProvider, GitRepository } from '@/lib/api/types'

export const GIT_PROVIDER_LABELS: Record<GitProvider, string> = {
  github: 'GitHub',
  gitlab: 'GitLab',
}

/** Web URL of each provider's public instance (`GitProvider::default_base_url`). */
export const GIT_DEFAULT_BASE_URLS: Record<GitProvider, string> = {
  github: 'https://github.com',
  gitlab: 'https://gitlab.com',
}

/** What a personal access token must be allowed to do, per provider. */
export const GIT_TOKEN_SCOPES: Record<GitProvider, string[]> = {
  github: ['repo'],
  gitlab: ['read_api', 'read_repository'],
}

/** What the OAuth application created on GitLab asks for (`ferry_scm::oauth::GITLAB_SCOPES`). */
export const GITLAB_APPLICATION_SCOPES = ['read_api', 'read_repository'] as const

/** How an account got connected, in a few words. */
export const GIT_AUTH_LABELS: Record<GitAuth, string> = {
  github_app: 'GitHub App',
  oauth: 'OAuth application',
  token: 'Access token',
}

/** The address typed for a self-hosted instance, as a URL (a bare host means https). */
export function instanceUrl(input: string): string {
  const u = input.trim().replace(/\/+$/, '')
  if (!u) return ''
  return u.includes('://') ? u : `https://${u}`
}

/** A message when `input` can't be the web URL of a provider instance (empty = the public one). */
export function instanceUrlError(input: string): string | null {
  const u = instanceUrl(input)
  if (!u) return null
  if (/[\s?#]/.test(u)) return 'Enter the address of the instance, e.g. https://gitlab.example.com.'
  return parseHttpUrl(u) ? null : 'Enter an http(s) address without credentials, e.g. https://gitlab.example.com.'
}

/**
 * The provider's page that creates a personal access token, with the name and
 * scopes Ferry needs filled in. `baseUrl` = a self-hosted instance.
 */
export function tokenPageUrl(provider: GitProvider, baseUrl?: string | null): string {
  const base = (baseUrl ? instanceUrl(baseUrl) : '') || GIT_DEFAULT_BASE_URLS[provider]
  const scopes = GIT_TOKEN_SCOPES[provider].join(',')
  return provider === 'github'
    ? `${base}/settings/tokens/new?description=Ferry&scopes=${scopes}`
    : `${base}/-/user_settings/personal_access_tokens?name=Ferry&scopes=${scopes}`
}

interface HttpUrl {
  scheme: 'http' | 'https'
  host: string
  port: number
  path: string
}

/**
 * `ferry_core::git::parse_http_url`: an http(s) URL that only says where to
 * go (no credentials, a plain host, an optional numeric port), else `null`.
 */
export function parseHttpUrl(url: string): HttpUrl | null {
  const m = /^(https?):\/\/([^/?#]*)([^?#]*)/i.exec(url.trim())
  if (!m) return null
  const scheme = (m[1] ?? '').toLowerCase() === 'https' ? 'https' : 'http'
  const authority = m[2] ?? ''
  const parts = /^(\[[0-9a-f:.]+\]|[a-z0-9][a-z0-9.-]*)(?::(\d{1,5}))?$/i.exec(authority)
  if (!parts) return null
  const port = parts[2] === undefined ? (scheme === 'https' ? 443 : 80) : Number(parts[2])
  if (port < 1 || port > 65535) return null
  return { scheme, host: (parts[1] ?? '').toLowerCase(), port, path: m[3] ?? '' }
}

/**
 * GitLab's page where the user creates the OAuth application this server
 * needs (GitLab has no way to register one on the fly).
 */
export function applicationsPageUrl(baseUrl?: string | null): string {
  const base = (baseUrl ? instanceUrl(baseUrl) : '') || GIT_DEFAULT_BASE_URLS.gitlab
  return `${base}/-/user_settings/applications`
}

/** The accounts repositories can be listed and cloned through (their authorization is finished). */
export function connectedAccounts(connections: readonly GitConnectionView[] | undefined): GitConnectionView[] {
  return (connections ?? []).filter((c) => c.status === 'connected')
}

/**
 * The OAuth application saved for a provider instance, if any: a later
 * authorization there needs no application typed again.
 */
export function savedApplication(
  connections: readonly GitConnectionView[] | undefined,
  provider: GitProvider,
  baseUrl: string,
): GitConnectionView | undefined {
  const base = baseUrl || GIT_DEFAULT_BASE_URLS[provider]
  const candidates = (connections ?? []).filter(
    (c) => c.provider === provider && c.base_url === base && c.auth === 'oauth' && c.client_id,
  )
  // The one that waits for its account first (the server looks it up first too).
  return candidates.find((c) => c.status === 'pending') ?? candidates[0]
}

/** `octocat`, or what a connection is while it has no account yet. */
export function accountName(connection: Pick<GitConnectionView, 'provider' | 'account'>): string {
  return connection.account || `${GIT_PROVIDER_LABELS[connection.provider]} application`
}

/** Host of a self-hosted instance (`gitlab.example.com`), `null` on the provider's public one. */
export function connectionHost(connection: Pick<GitConnectionView, 'provider' | 'base_url'>): string | null {
  if (connection.base_url === GIT_DEFAULT_BASE_URLS[connection.provider]) return null
  return connection.base_url.replace(/^https?:\/\//i, '')
}

/** `octocat`, or `octocat · gitlab.example.com` on a self-hosted instance. */
export function connectionLabel(connection: Pick<GitConnectionView, 'provider' | 'base_url' | 'account'>): string {
  const host = connectionHost(connection)
  return host ? `${accountName(connection)} · ${host}` : accountName(connection)
}

/**
 * What the token can't do that Ferry needs, from the scopes the provider
 * reported (`null` when fine, or when the provider reports none — GitHub's
 * fine-grained tokens).
 */
export function scopeWarning(connection: Pick<GitConnectionView, 'provider' | 'scopes'>): string | null {
  const scopes = connection.scopes
  if (scopes.length === 0) return null
  if (connection.provider === 'github') {
    return scopes.includes('repo') ? null : 'Without the repo scope, this token only sees public repositories.'
  }
  const clones = scopes.some((s) => s === 'read_repository' || s === 'write_repository' || s === 'api')
  return clones ? null : 'Without the read_repository scope, this token can’t clone private repositories.'
}

export type TokenExpiry = { tone: 'ok' | 'soon' | 'expired'; label: string }

const DAY_MS = 86_400_000

/** When the token expires, for display: `Expires in 12 days`, `Expired 3 days ago`. `null` = no expiry known. */
export function tokenExpiry(expiresAt: string | null | undefined, now: number = Date.now()): TokenExpiry | null {
  if (!expiresAt) return null
  const at = new Date(expiresAt).getTime()
  if (Number.isNaN(at)) return null
  const days = Math.floor(Math.abs(at - now) / DAY_MS)
  const span = days === 0 ? 'today' : days === 1 ? '1 day' : `${days} days`
  if (at <= now) return { tone: 'expired', label: days === 0 ? 'Expired today' : `Expired ${span} ago` }
  const label = days === 0 ? 'Expires today' : `Expires in ${span}`
  return { tone: days < 14 ? 'soon' : 'ok', label }
}

/**
 * Repositories matching every word of `query` (case-insensitive, anywhere in
 * the full name), in the given order.
 */
export function filterRepositories(repositories: readonly GitRepository[], query: string): GitRepository[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (words.length === 0) return [...repositories]
  return repositories.filter((r) => {
    const name = r.full_name.toLowerCase()
    return words.every((w) => name.includes(w))
  })
}

/**
 * A service name for a repository: lowercase letters, digits and `-`,
 * starting with a letter, at most 40 characters (`validate::resource_name`).
 * Empty when nothing usable is left.
 */
export function serviceNameFromRepository(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^[^a-z]+/, '')
    .slice(0, 40)
    .replace(/-+$/, '')
  return slug
}

/** Whether `name` can be a git branch (`validate::branch`). */
export function isBranchName(name: string): boolean {
  const v = name.trim()
  return !(
    v === '' ||
    v.length > 255 ||
    v.startsWith('-') ||
    v.startsWith('/') ||
    v.endsWith('/') ||
    v.endsWith('.') ||
    v.endsWith('.lock') ||
    v.includes('..') ||
    v.includes('//') ||
    v.includes('@{') ||
    /[\s\p{Cc}~^:?*[\\]/u.test(v)
  )
}

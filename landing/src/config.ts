/**
 * Where the docs site (../docs, a static export) is served. Set VITE_DOCS_URL
 * when building for a public site; the default is the docs' local dev server.
 */
const docsBase = (import.meta.env.VITE_DOCS_URL ?? 'http://localhost:3000').replace(/\/+$/, '')

/** A docs page, e.g. `docs('guides/production')`. Static-export URLs end with a slash. */
export const docs = (page = '') => `${docsBase}/docs/${page ? `${page}/` : ''}`

export const githubUrl = 'https://github.com/Carter2307/ferry'

export const links = {
  quickstart: docs('getting-started/quickstart'),
  installation: docs('getting-started/installation'),
  cli: docs('reference/cli'),
  api: docs('reference/api-overview'),
  blueprint: docs('reference/blueprint'),
  deploys: docs('concepts/deploys'),
  builds: docs('concepts/builds'),
  services: docs('concepts/services'),
  datastores: docs('concepts/datastores'),
  networking: docs('concepts/networking'),
  jobs: docs('concepts/jobs'),
  logs: docs('concepts/logs'),
  production: docs('guides/production'),
  githubAutoDeploy: docs('guides/github-auto-deploy'),
  postgresApp: docs('guides/postgres-app'),
  migrate: docs('guides/migrate-from-render'),
  architecture: docs('architecture/overview'),
  contributing: docs('contributing/development'),
  license: `${githubUrl}/blob/main/LICENSE`,
}

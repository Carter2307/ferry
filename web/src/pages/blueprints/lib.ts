import type { BlueprintAction } from '@/lib/api/types'

/** create / update / unchanged counts of a plan. */
export function countActions(actions: BlueprintAction[]) {
  const c = { create: 0, update: 0, unchanged: 0 }
  for (const a of actions) {
    if (a.action === 'create') c.create++
    else if (a.action === 'update') c.update++
    else c.unchanged++
  }
  return c
}

/** "… at line 12 column 3 …" in a YAML parse error → {line, column}. */
export function errorLocation(message: string): { line: number; column: number | null } | null {
  const m = /\bline (\d+)(?:,?\s*column (\d+))?/i.exec(message)
  if (!m?.[1]) return null
  const line = Number(m[1])
  if (!Number.isFinite(line) || line < 1) return null
  return { line, column: m[2] ? Number(m[2]) : null }
}

/** Largest file accepted by the picker (the YAML is sent as one JSON string). */
export const MAX_BLUEPRINT_BYTES = 1024 * 1024

export const EXAMPLE_BLUEPRINT = `# ferry.yaml — Render-compatible blueprint.
# Dry run first: nothing is written until you apply. Nothing is ever deleted.

envVarGroups:
  - name: my-app-settings
    envVars:
      - key: LOG_LEVEL
        value: info
      - key: SESSION_SECRET
        generateValue: true

databases:
  - name: my-app-db
    postgresMajorVersion: "16"

services:
  - type: web
    name: my-app
    image:
      url: nginx:alpine
    port: 80
    healthCheckPath: /
    numInstances: 1
    envVars:
      - fromGroup: my-app-settings
      - key: DATABASE_URL
        fromDatabase:
          name: my-app-db
          property: connectionString
`

import { Boxes, Braces, Database, FileCode2, Server, type LucideIcon } from 'lucide-react'

export interface NavItem {
  label: string
  to: string
  icon: LucideIcon
  /** Short description (command palette). */
  hint: string
}

/** Primary navigation (icon rail, mobile sheet, command palette). */
export const NAV_ITEMS: NavItem[][] = [
  [{ label: 'Services', to: '/services', icon: Boxes, hint: 'Web services, workers, cron jobs, static sites' }],
  [
    { label: 'Datastores', to: '/datastores', icon: Database, hint: 'Managed Postgres and Redis' },
    { label: 'Env groups', to: '/env-groups', icon: Braces, hint: 'Shared environment variables' },
    { label: 'Blueprints', to: '/blueprints', icon: FileCode2, hint: 'Apply ferry.yaml / render.yaml' },
  ],
  [{ label: 'Server', to: '/server', icon: Server, hint: 'Ferry and Docker versions, proxy' }],
]

export const REPO_URL = 'https://github.com/Carter2307/ferry'
export const DOCS_URL = `${REPO_URL}#readme`
export const DESIGN_DOC_URL = `${REPO_URL}/blob/main/DESIGN.md`
export { API_DOCS_URL, OPENAPI_URL } from '@/lib/api/queries/info'

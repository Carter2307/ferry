import * as React from 'react'
import { useSearchParams } from 'react-router'
import { Code2, Plug, Settings2, UserRound } from 'lucide-react'

import { PageContainer } from '@/components/patterns/Page'
import { API_DOCS_URL, DESIGN_DOC_URL, DOCS_URL, REPO_URL } from '@/components/shell/nav'
import { useOpenApiSpec } from '@/lib/api/queries'

import { AccountSection } from './AccountSection'
import { ServerMenu, type ServerMenuItem } from './ServerMenu'
import { ApiSection, ConnectionsSection, GeneralSection } from './sections'

const SECTIONS: (ServerMenuItem & { Component: React.ComponentType })[] = [
  { id: 'general', label: 'General', icon: <Settings2 />, Component: GeneralSection },
  { id: 'account', label: 'Account', icon: <UserRound />, Component: AccountSection },
  { id: 'connections', label: 'Connections', icon: <Plug />, Component: ConnectionsSection },
  { id: 'api', label: 'API', icon: <Code2 />, Component: ApiSection },
]

const LINKS = [
  { label: 'Documentation', href: DOCS_URL },
  { label: 'API docs', href: API_DOCS_URL },
  { label: 'Design notes', href: DESIGN_DOC_URL },
  { label: 'Source code', href: REPO_URL },
]

/**
 * `/server` — Studio "Settings"-style page with an inner menu:
 * General (server info + theme), Account (password, API tokens, sessions),
 * Connections (CLI, git accounts, GitHub webhook), API.
 * The section lives in `?section=` (the route has no sub-paths).
 */
export function ServerPage() {
  const [params] = useSearchParams()
  const requested = params.get('section')
  const section = SECTIONS.find((s) => s.id === requested) ?? SECTIONS[0]
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  // Only link to the API docs when this server publishes them.
  const docs = useOpenApiSpec()
  const links = docs.available ? LINKS : LINKS.filter((l) => l.href !== API_DOCS_URL)

  React.useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [section?.id])

  if (!section) return null
  const { Component } = section

  return (
    <div className="flex min-h-0 flex-1 flex-col md:flex-row">
      <ServerMenu items={SECTIONS} links={links} current={section.id} />
      <div ref={scrollRef} className="relative min-h-0 min-w-0 flex-1 overflow-y-auto">
        <PageContainer size="narrow">
          <Component />
        </PageContainer>
      </div>
    </div>
  )
}

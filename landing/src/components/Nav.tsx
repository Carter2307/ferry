import { siGithub } from 'simple-icons'

import { docs, githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'
import { Wordmark } from './FerryMark'

const LINKS = [
  { label: 'Product', href: '#product' },
  { label: 'Docs', href: docs() },
  { label: 'Blueprints', href: links.blueprint },
  { label: 'API', href: links.api },
]

export function Nav() {
  return (
    <header className="sticky top-0 z-30 border-b border-border bg-background/85 backdrop-blur-md">
      <div className="container-page flex h-16 items-center gap-8">
        <a href="/" aria-label="Ferry home" className="rounded-md">
          <Wordmark />
        </a>
        <nav aria-label="Main" className="hidden items-center gap-1 md:flex">
          {LINKS.map((l) => (
            <a
              key={l.label}
              href={l.href}
              className="rounded-md px-3 py-1.5 text-sm font-medium text-foreground-light transition-colors hover:text-foreground"
            >
              {l.label}
            </a>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-2">
          <a href={githubUrl} className="btn btn-secondary btn-sm" aria-label="Ferry on GitHub">
            <BrandIcon icon={siGithub} className="size-4" />
            <span className="max-sm:hidden">GitHub</span>
          </a>
          <a href={links.quickstart} className="btn btn-primary btn-sm">
            Get started
          </a>
        </div>
      </div>
    </header>
  )
}

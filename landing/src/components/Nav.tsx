import { useSyncExternalStore } from 'react'
import { siGithub } from 'simple-icons'

import { docs, githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'
import { Wordmark } from './FerryMark'
import { ThemeToggle } from './ThemeToggle'

const LINKS = [
  { label: 'Product', href: '#product' },
  { label: 'Docs', href: docs() },
  { label: 'Blueprints', href: links.blueprint },
  { label: 'API', href: links.api },
]

const subscribeScroll = (onChange: () => void) => {
  window.addEventListener('scroll', onChange, { passive: true })
  return () => window.removeEventListener('scroll', onChange)
}

/**
 * The header: transparent, with a blur of what is behind it and a hairline at its bottom edge. At
 * the top of the page the pixel field shows through it. After the first scroll it takes a light
 * tint, so the links stay easy to read over the content that passes under them.
 */
export function Nav() {
  const scrolled = useSyncExternalStore(subscribeScroll, () => window.scrollY > 8)
  return (
    <header
      data-scrolled={scrolled ? '' : undefined}
      className="sticky top-0 z-30 border-b border-border bg-transparent backdrop-blur-md transition-[background-color] duration-300 data-[scrolled]:bg-background/45"
    >
      <div className="container-page flex h-16 items-center gap-6">
        <a href="/" aria-label="Ferry home" className="rounded-md">
          <Wordmark />
        </a>
        <nav aria-label="Main" className="hidden items-center gap-0.5 md:flex">
          {LINKS.map((l) => (
            <a key={l.label} href={l.href} className="btn btn-ghost btn-sm">
              {l.label}
            </a>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-1.5">
          <a href={githubUrl} className="btn btn-ghost btn-sm btn-icon" aria-label="Ferry on GitHub">
            <BrandIcon icon={siGithub} />
          </a>
          <ThemeToggle />
          <a href={links.quickstart} className="btn btn-primary btn-sm ml-1.5">
            Get started
          </a>
        </div>
      </div>
    </header>
  )
}

import { siGithub } from 'simple-icons'

import { docs, githubUrl, links } from '@/config'

import { Divider } from './backdrop'
import { BrandIcon } from './BrandIcon'
import { Wordmark } from './FerryMark'
import { ThemeToggle } from './ThemeToggle'

const COLUMNS = [
  {
    title: 'Product',
    links: [
      { label: 'Deploys', href: links.deploys },
      { label: 'Services', href: links.services },
      { label: 'Postgres & Redis', href: links.datastores },
      { label: 'Domains & HTTPS', href: links.networking },
      { label: 'Cron jobs', href: links.jobs },
      { label: 'Blueprints', href: links.blueprint },
    ],
  },
  {
    title: 'Docs',
    links: [
      { label: 'Introduction', href: docs() },
      { label: 'Quickstart', href: links.quickstart },
      { label: 'Installation', href: links.installation },
      { label: 'CLI reference', href: links.cli },
      { label: 'API reference', href: links.api },
    ],
  },
  {
    title: 'Guides',
    links: [
      { label: 'Going to production', href: links.production },
      { label: 'Auto-deploy from GitHub', href: links.githubAutoDeploy },
      { label: 'An app with Postgres', href: links.postgresApp },
      { label: 'Moving from Render', href: links.migrate },
    ],
  },
  {
    title: 'Project',
    links: [
      { label: 'GitHub', href: githubUrl },
      { label: 'Architecture', href: links.architecture },
      { label: 'Contributing', href: links.contributing },
      { label: 'MIT license', href: links.license },
    ],
  },
]

export function Footer() {
  return (
    // No background: the pixel field of the end of the page shows through the footer.
    <footer>
      <Divider delay="-5s" />
      <div className="container-page grid gap-12 py-16 lg:grid-cols-[minmax(0,2fr)_repeat(4,minmax(0,1fr))]">
        <div>
          <a href="/" aria-label="Ferry home" className="inline-block rounded-md">
            <Wordmark />
          </a>
          <p className="mt-4 max-w-[17rem] text-sm text-foreground-lighter">
            A self-hosted Render alternative, written in Rust.
          </p>
          <a
            href={githubUrl}
            aria-label="Ferry on GitHub"
            className="mt-5 inline-grid size-8 place-items-center rounded-md text-foreground-lighter transition-colors hover:text-foreground"
          >
            <BrandIcon icon={siGithub} className="size-5" />
          </a>
        </div>
        <div className="grid grid-cols-2 gap-10 sm:grid-cols-4 lg:col-span-4">
          {COLUMNS.map((col) => (
            <nav key={col.title} aria-label={col.title}>
              <h2 className="font-sans text-sm font-medium text-foreground">{col.title}</h2>
              <ul className="mt-4 space-y-2.5 text-sm">
                {col.links.map((l) => (
                  <li key={l.label}>
                    <a href={l.href} className="text-foreground-lighter transition-colors hover:text-foreground">
                      {l.label}
                    </a>
                  </li>
                ))}
              </ul>
            </nav>
          ))}
        </div>
      </div>
      <div className="container-page flex items-center justify-between border-t border-border py-5 text-[13px] text-foreground-lighter">
        <span>Ferry is open source under the MIT license.</span>
        <ThemeToggle />
      </div>
      {/* A free band at the end of the page: the pixel field (PageEndBackdrop) is dense here. */}
      <div aria-hidden="true" className="h-36 sm:h-48" />
    </footer>
  )
}

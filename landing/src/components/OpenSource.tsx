import { siGithub } from 'simple-icons'

import { githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'
import { Heading, Section } from './ui'

/** The Rust workspace (DESIGN.md §2). */
const CRATES = [
  { name: 'ferryd', text: 'The server: wires everything together' },
  { name: 'ferry-api', text: 'REST API, live events, OpenAPI, the dashboard' },
  { name: 'ferry-engine', text: 'Deploys, the reconciler, cron, datastores, logs' },
  { name: 'ferry-build', text: 'Git fetch, runtime detection, image builds' },
  { name: 'ferry-proxy', text: 'Reverse proxy with HTTP/2 and WebSockets' },
  { name: 'ferry-tls', text: 'Certificates from Let’s Encrypt' },
  { name: 'ferry-docker', text: 'A typed Docker client' },
  { name: 'ferry-core', text: 'Models, the SQLite store, validation' },
  { name: 'ferry-cli', text: 'The ferry command' },
]

export function OpenSource() {
  return (
    <Section labelledBy="oss-title" className="grid gap-12 lg:grid-cols-2 lg:gap-16">
      <div>
        <Heading id="oss-title" strong="Open source, MIT licensed" quiet="Read it, run it, change it" />
        <p className="mt-5 max-w-[28rem] text-foreground-lighter">
          Ferry is built in the open, in Rust. Your apps run as plain Docker containers and your data stays in Docker
          volumes on your own machine.
        </p>
        <div className="mt-8 flex flex-wrap items-center gap-5">
          <a className="btn btn-secondary" href={githubUrl}>
            <BrandIcon icon={siGithub} />
            View on GitHub
          </a>
          <a className="text-link text-sm" href={links.architecture}>
            How it’s built
          </a>
        </div>
      </div>

      <div className="overflow-hidden rounded-xl border border-border-strong bg-surface-100">
        <div className="flex h-11 items-center border-b border-border px-5 text-[13px] text-foreground-lighter">
          Nine crates, one workspace
        </div>
        <ul>
          {CRATES.map((c) => (
            <li
              key={c.name}
              className="grid grid-cols-[8.5rem_minmax(0,1fr)] items-baseline gap-4 border-b border-border px-5 py-2.5 last:border-b-0"
            >
              <code className="text-[12.5px] text-foreground">{c.name}</code>
              <span className="truncate text-sm text-foreground-lighter">{c.text}</span>
            </li>
          ))}
        </ul>
      </div>
    </Section>
  )
}

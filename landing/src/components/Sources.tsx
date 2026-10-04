import { FolderOpen, GitBranch, type LucideIcon } from 'lucide-react'
import { useRef, useState, type KeyboardEvent } from 'react'
import { siDocker } from 'simple-icons'

import { links } from '@/config'

import { BrandIcon } from './BrandIcon'
import { CopyButton, Transcript } from './code'
import { Reveal } from './Reveal'
import { Heading, Section } from './ui'

// Output as the ferry CLI prints it (crates/ferry-cli, crates/ferry-engine, crates/ferry-build).
const SOURCES = [
  {
    id: 'git',
    tab: 'Git repository',
    heading: 'a git repository',
    icon: GitBranch as LucideIcon,
    text: 'GitHub, GitLab, any SSH URL or a local path. Add a GitHub webhook and every push to the branch deploys.',
    command: 'ferry create api --repo https://github.com/you/api --branch main --follow',
    output: [
      "Created web service 'api' (srv-8k2m4x7q1c9w5v3b6n0t)",
      'URL: https://api.example.com',
      'Deploy dep-c5n8x1q7k2w9v4b6m3ta queued (create), streaming logs…',
      '==> Cloning from https://github.com/you/api (branch main)',
      '==> Checked out 7c1e2f4: Add a health check',
      '==> Detected Node.js runtime',
      '==> Build successful 🎉',
      '==> Health check passed',
      '==> Your service is live 🎉',
      '==> Available at https://api.example.com',
    ],
  },
  {
    id: 'folder',
    tab: 'Local folder',
    heading: 'a folder on your laptop',
    icon: FolderOpen as LucideIcon,
    text: 'No commit and no registry. Ferry packs the folder, leaves out what .gitignore lists, uploads it and deploys it.',
    command: 'ferry up --follow',
    output: [
      'Packing /home/you/my-app …',
      'Packed 38 file(s), 96.2 KiB',
      "Created web service 'my-app' (srv-2t7w9k4m1q8c5v3b6x0n)",
      'Uploading 96.2 KiB …',
      'Deploy dep-q4m8x2k7c1w9v5b3n6ta queued (upload), streaming logs…',
      '==> Extracting uploaded source archive',
      '==> Detected Node.js runtime',
      '==> Build successful 🎉',
      '==> Your service is live 🎉',
      '==> Available at https://my-app.example.com',
    ],
  },
  {
    id: 'image',
    tab: 'Docker image',
    heading: 'a Docker image',
    icon: null,
    text: 'Run an image from a public registry as it is, with the same URL, logs and scaling as the rest.',
    command: 'ferry create hello --image nginx:alpine --port 80 --follow',
    output: [
      "Created web service 'hello' (srv-6v1n3x8k2w7q4c9m5b0t)",
      'URL: https://hello.example.com',
      'Deploy dep-w3k7m1x9q5c2v8b4n6ta queued (create), streaming logs…',
      '==> Pulling image nginx:alpine',
      '==> Starting 1 instance(s)',
      '==> Health check passed',
      '==> Your service is live 🎉',
      '==> Available at https://hello.example.com',
    ],
  },
]

export function Sources() {
  const [index, setIndex] = useState(0)
  const source = SOURCES[index]!
  const tabs = useRef<(HTMLButtonElement | null)[]>([])

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const next = (i + step + SOURCES.length) % SOURCES.length
    setIndex(next)
    tabs.current[next]?.focus()
  }

  return (
    <Section labelledBy="sources-title" beam="-9s" className="grid gap-12 lg:grid-cols-2 lg:gap-16">
      <div>
        <Heading id="sources-title" quietFirst quiet="Deploy from" strong={source.heading} />
        <Reveal delay={0.12}>
          <p className="mt-5 max-w-[28rem] text-foreground-lighter">{source.text}</p>
          <p className="mt-4 max-w-[28rem] text-foreground-lighter">
            No Dockerfile needed: Ferry detects Node.js, Python, Go, Rust, Ruby and static sites, and uses your
            Dockerfile when there is one.
          </p>
          <a className="text-link mt-6 inline-block text-sm" href={links.builds}>
            How builds work
          </a>
        </Reveal>
      </div>

      <Reveal
        delay={0.1}
        y={18}
        className="min-w-0 overflow-hidden rounded-xl border border-border-strong bg-surface-100"
      >
        <div role="tablist" aria-label="Where the code comes from" className="grid grid-cols-3 border-b border-border">
          {SOURCES.map((s, i) => {
            const Icon = s.icon
            return (
              <button
                key={s.id}
                ref={(el) => {
                  tabs.current[i] = el
                }}
                type="button"
                role="tab"
                id={`source-tab-${s.id}`}
                aria-selected={i === index}
                aria-controls="source-panel"
                tabIndex={i === index ? 0 : -1}
                onClick={() => setIndex(i)}
                onKeyDown={(e) => onKeyDown(e, i)}
                className={`flex h-12 items-center justify-center gap-2 border-r border-border text-[13px] font-medium transition-colors last:border-r-0 ${
                  i === index
                    ? 'bg-surface-300 text-foreground'
                    : 'text-foreground-lighter hover:bg-surface-200 hover:text-foreground'
                }`}
              >
                {Icon ? (
                  <Icon className="size-4" aria-hidden="true" />
                ) : (
                  <BrandIcon icon={siDocker} className="size-4" />
                )}
                <span className="max-sm:sr-only">{s.tab}</span>
              </button>
            )
          })}
        </div>
        <div
          role="tabpanel"
          id="source-panel"
          aria-labelledby={`source-tab-${source.id}`}
          className="relative min-h-[23rem] bg-log"
        >
          <Transcript command={source.command} output={source.output} />
          <CopyButton text={source.command} label="Copy the command" className="absolute top-2.5 right-2.5" />
        </div>
      </Reveal>
    </Section>
  )
}

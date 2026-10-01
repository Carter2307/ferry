import { githubUrl, links } from '@/config'

import { CommandBlock } from './code'
import { Heading, Section } from './ui'

const SERVER = [
  `git clone ${githubUrl} && cd ferry`,
  '(cd web && npm ci && npm run build)',
  'cargo build --release',
  './target/release/ferryd',
]

const FIRST_APP = [
  'export PATH="$PWD/target/release:$PATH"',
  'ferry login',
  'cd examples/node-hello && ferry up --follow',
  'curl http://node-hello.localhost:8080',
]

export function Install() {
  return (
    <Section id="install" labelledBy="install-title">
      <div className="grid gap-x-16 gap-y-12 lg:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
        <div className="min-w-0">
          <Heading id="install-title" strong="Try it on your own machine" quiet="then on any Linux server" />
          <p className="mt-5 max-w-[34rem] text-foreground-lighter">
            You need Docker, and Docker Desktop is fine on a Mac. To build from source you also need Rust 1.89 or later
            and Node.js 20 or later.
          </p>

          <ol className="mt-10 space-y-8">
            <li>
              <h3 className="text-base font-semibold">Build and start the server</h3>
              <p className="mt-1 mb-3 text-sm text-foreground-lighter">
                <code className="inline-code">ferryd</code> starts in the background and prints a link: open it to create
                your account. <code className="inline-code">ferryd stop</code> stops the server.
              </p>
              <CommandBlock lines={SERVER} label="Copy the server commands" />
            </li>
            <li>
              <h3 className="text-base font-semibold">Deploy the example app</h3>
              <p className="mt-1 mb-3 text-sm text-foreground-lighter">
                <code className="inline-code">ferry login</code> opens the dashboard, where you approve the terminal.
                The dashboard is at <code className="inline-code">http://127.0.0.1:7878</code>.
              </p>
              <CommandBlock lines={FIRST_APP} label="Copy the deploy commands" />
            </li>
          </ol>
        </div>

        <div className="space-y-3 lg:pt-[7.5rem]">
          <div className="rounded-xl border border-border bg-surface-100 p-6">
            <h3 className="text-base font-semibold">Put it on a server</h3>
            <p className="mt-2 text-sm text-foreground-lighter">
              On a Linux machine, run <code className="inline-code">ferryd</code> under systemd, point a wildcard DNS
              record at it and give it your domain. Add an email address and it gets HTTPS certificates from Let’s
              Encrypt.
            </p>
            <a className="text-link mt-4 inline-block text-sm" href={links.production}>
              Read the production guide
            </a>
          </div>
          <div className="rounded-xl border border-border bg-surface-100 p-6">
            <h3 className="text-base font-semibold">Click, type or script it</h3>
            <p className="mt-2 text-sm text-foreground-lighter">
              Manage everything from the web dashboard, the <code className="inline-code">ferry</code> CLI or the REST
              API. The API is described in OpenAPI 3.1, and each server has its own Swagger UI at{' '}
              <code className="inline-code">/api/docs</code>.
            </p>
            <a className="text-link mt-4 inline-block text-sm" href={links.api}>
              Browse the API reference
            </a>
          </div>
        </div>
      </div>
    </Section>
  )
}

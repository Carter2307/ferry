import Link from 'next/link';
import type { Metadata } from 'next';
import {
  ArrowRight,
  BookOpen,
  Braces,
  CalendarClock,
  Database,
  FileCode,
  GitBranch,
  Globe,
  HeartPulse,
  LayoutDashboard,
  Rocket,
  Terminal,
  Wrench,
} from 'lucide-react';
import type { ReactNode } from 'react';
import { appDescription } from '@/lib/shared';

export const metadata: Metadata = {
  title: { absolute: 'Ferry Docs — your own Render' },
  description: appDescription,
};

const FEATURES: { icon: ReactNode; title: string; text: ReactNode; href: string }[] = [
  {
    icon: <Rocket />,
    title: 'Zero-downtime deploys',
    text: 'Blue/green deploys with health checks, deploy history, one-click rollbacks and cancel.',
    href: '/docs/concepts/deploys',
  },
  {
    icon: <GitBranch />,
    title: 'Git, folders or images',
    text: (
      <>
        Any git URL (auto-deploy on GitHub push, deploy hooks for anything else), <code>ferry up</code> from a local
        folder, or a prebuilt Docker image.
      </>
    ),
    href: '/docs/concepts/builds',
  },
  {
    icon: <Globe />,
    title: 'Networking & HTTPS',
    text: 'A built-in reverse proxy, a URL per service, custom domains with Let’s Encrypt and a private network.',
    href: '/docs/concepts/networking',
  },
  {
    icon: <Database />,
    title: 'Managed datastores',
    text: 'Postgres and Redis with persistent volumes, wired into services with env var references.',
    href: '/docs/concepts/datastores',
  },
  {
    icon: <CalendarClock />,
    title: 'Cron & one-off jobs',
    text: (
      <>
        Cron jobs on a schedule, and <code>ferry run</code> for one-off commands such as migrations.
      </>
    ),
    href: '/docs/concepts/jobs',
  },
  {
    icon: <FileCode />,
    title: 'Blueprints',
    text: (
      <>
        Describe everything in <code>ferry.yaml</code> — or apply your existing <code>render.yaml</code>.
      </>
    ),
    href: '/docs/reference/blueprint',
  },
  {
    icon: <HeartPulse />,
    title: 'Self-healing',
    text: 'A reconciler replaces crashed containers, enforces instance counts and keeps routes current.',
    href: '/docs/concepts/self-healing',
  },
  {
    icon: <LayoutDashboard />,
    title: 'Dashboard, CLI & API',
    text: (
      <>
        A web dashboard, the <code>ferry</code> CLI and a REST API described by OpenAPI 3.1.
      </>
    ),
    href: '/docs/getting-started/dashboard',
  },
];

const SECTIONS = [
  {
    icon: <BookOpen />,
    label: 'Getting started',
    title: 'Install Ferry and deploy your first app',
    href: '/docs/getting-started/installation',
  },
  { icon: <Wrench />, label: 'Guides', title: 'Auto-deploy from GitHub, run Postgres, go to production', href: '/docs/guides/github-auto-deploy' },
  { icon: <Terminal />, label: 'Reference', title: 'CLI, server options, blueprint spec', href: '/docs/reference/cli' },
  { icon: <Braces />, label: 'API', title: 'Every endpoint of the REST API', href: '/docs/reference/api' },
];

function Prompt({ children }: { children: ReactNode }) {
  return (
    <div>
      <span className="text-foreground-muted select-none">$ </span>
      {children}
    </div>
  );
}

function Comment({ children }: { children: ReactNode }) {
  return <div className="text-foreground-lighter"># {children}</div>;
}

export default function HomePage() {
  return (
    <div className="mx-auto w-full max-w-[1120px] px-4 md:px-8">
      {/* Hero */}
      <section className="grid items-center gap-10 py-14 md:py-20 lg:grid-cols-[1.05fr_1fr] lg:gap-14">
        <div>
          <p className="mono-label mb-4">Self-hosted · Rust · MIT</p>
          <h1 className="text-[38px] leading-[1.1] font-medium tracking-[-0.025em] text-foreground sm:text-[48px]">
            Ferry — your own Render
          </h1>
          <p className="mt-5 max-w-[34rem] text-[17px] leading-relaxed text-foreground-light">
            Turn one machine with Docker into your own Render. Point Ferry at a git repo, a local folder or a Docker
            image: it builds the code, deploys it with zero downtime and gives it a URL — then scales, heals and logs
            your apps.
          </p>
          <div className="mt-8 flex flex-wrap gap-3">
            <Link
              href="/docs/getting-started/quickstart"
              className="inline-flex h-9 items-center gap-2 rounded-[var(--ferry-radius-md)] border border-primary-solid-border bg-primary-solid px-4 text-[14px] font-medium text-primary-foreground transition-colors hover:brightness-110"
            >
              Get started
              <ArrowRight className="size-4" aria-hidden="true" />
            </Link>
            <Link
              href="/docs"
              className="inline-flex h-9 items-center gap-2 rounded-[var(--ferry-radius-md)] border border-border-strong bg-surface-100 px-4 text-[14px] font-medium text-foreground transition-colors hover:border-border-stronger hover:bg-surface-200"
            >
              Read the docs
            </Link>
          </div>
        </div>

        <div className="min-w-0 overflow-hidden rounded-[var(--ferry-radius-lg)] border border-border bg-surface-100 shadow-card">
          <div className="flex items-center justify-between border-b border-border px-4 py-2.5">
            <span className="mono-label">Quick start</span>
            <span className="flex gap-1.5" aria-hidden="true">
              <span className="size-2 rounded-full bg-border-stronger" />
              <span className="size-2 rounded-full bg-border-stronger" />
              <span className="size-2 rounded-full bg-border-stronger" />
            </span>
          </div>
          <pre className="overflow-x-auto bg-surface-75 px-4 py-4 font-mono text-[12.5px] leading-[1.75] text-foreground">
            <code>
              <Comment>get the code, build the dashboard, then the server and the CLI</Comment>
              <Prompt>git clone https://github.com/Carter2307/ferry &amp;&amp; cd ferry</Prompt>
              <Prompt>(cd web &amp;&amp; npm ci &amp;&amp; npm run build)</Prompt>
              <Prompt>cargo build --release</Prompt>
              <Prompt>./target/release/ferryd</Prompt>
              <div>&nbsp;</div>
              <Comment>in another terminal, with the token ferryd printed</Comment>
              <Prompt>export PATH=&quot;$PWD/target/release:$PATH&quot;</Prompt>
              <Prompt>ferry login --server http://127.0.0.1:7878 --token &lt;token&gt;</Prompt>
              <Prompt>cd examples/node-hello &amp;&amp; ferry up --follow</Prompt>
              <Prompt>curl http://node-hello.localhost:8080</Prompt>
            </code>
          </pre>
        </div>
      </section>

      {/* Features */}
      <section className="border-t border-border py-14" aria-labelledby="features">
        <p className="mono-label mb-2">What Ferry runs</p>
        <h2 id="features" className="text-[24px] font-medium tracking-[-0.015em] text-foreground">
          Render’s building blocks, on your own server
        </h2>
        <p className="mt-2 max-w-2xl text-[15px] text-foreground-light">
          Web services, private services, background workers, cron jobs and static sites — plus everything around them.
          Docker is the only runtime dependency.
        </p>
        <div className="mt-8 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {FEATURES.map((f) => (
            <Link
              key={f.title}
              href={f.href}
              className="group flex flex-col rounded-[var(--ferry-radius-lg)] border border-border bg-surface-100 p-4 shadow-card transition-colors hover:border-border-stronger"
            >
              <span className="mb-4 inline-flex size-9 items-center justify-center rounded-[var(--ferry-radius-md)] border border-border-strong bg-surface-200 text-foreground-light transition-colors group-hover:text-primary [&_svg]:size-[18px]">
                {f.icon}
              </span>
              <span className="text-[14.5px] font-medium text-foreground">{f.title}</span>
              <span className="mt-1.5 text-[13.5px] leading-relaxed text-foreground-light [&_code]:rounded-[var(--ferry-radius-sm)] [&_code]:border [&_code]:border-border [&_code]:bg-surface-200 [&_code]:px-1 [&_code]:font-mono [&_code]:text-[12px]">
                {f.text}
              </span>
            </Link>
          ))}
        </div>
      </section>

      {/* Sections */}
      <section className="border-t border-border py-14" aria-labelledby="explore">
        <p className="mono-label mb-2">Documentation</p>
        <h2 id="explore" className="text-[24px] font-medium tracking-[-0.015em] text-foreground">
          Explore the docs
        </h2>
        <div className="mt-8 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {SECTIONS.map((s) => (
            <Link
              key={s.label}
              href={s.href}
              className="group flex items-start gap-3 rounded-[var(--ferry-radius-lg)] border border-border bg-surface-100 p-4 transition-colors hover:border-border-stronger"
            >
              <span className="mt-0.5 text-foreground-lighter transition-colors group-hover:text-primary [&_svg]:size-[18px]">
                {s.icon}
              </span>
              <span className="min-w-0 flex-1">
                <span className="mono-label block">{s.label}</span>
                <span className="mt-1 block text-[14px] leading-snug text-foreground">{s.title}</span>
              </span>
              <ArrowRight
                className="mt-0.5 size-4 shrink-0 text-foreground-muted transition-transform group-hover:translate-x-0.5 group-hover:text-foreground-light"
                aria-hidden="true"
              />
            </Link>
          ))}
        </div>
      </section>
    </div>
  );
}

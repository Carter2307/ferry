import {
  Boxes,
  CalendarClock,
  Check,
  Database,
  FileCode2,
  Globe,
  Rocket,
  ScrollText,
  type LucideIcon,
} from 'lucide-react'
import type { ReactNode } from 'react'

import { links } from '@/config'

import { ArtBlueprint, ArtCron, ArtDatastores, ArtDomains, ArtFerry, ArtLogs, ArtServices } from './art'
import { Emphasis } from './ui'

function Card({
  icon: Icon,
  title,
  text,
  href,
  className = '',
  children,
}: {
  icon: LucideIcon
  title: string
  text: string
  href: string
  className?: string
  children: ReactNode
}) {
  return (
    <a href={href} className={`group card-frame block h-[22rem] hover:shadow-lg sm:h-[25rem] ${className}`}>
      <div className="card-panel p-6">
        <div className="relative z-10 flex items-center gap-2">
          <Icon className="size-4 text-foreground-light" aria-hidden="true" />
          <h3 className="text-base font-semibold">{title}</h3>
        </div>
        <p className="relative z-10 mt-3 max-w-[19rem] text-sm text-foreground-lighter">
          <Emphasis text={text} />
        </p>
        {children}
      </div>
    </a>
  )
}

export function Features() {
  return (
    <section id="product" aria-labelledby="product-title" className="container-page pb-20 lg:pb-28">
      <h2 id="product-title" className="sr-only">
        What Ferry runs
      </h2>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-12 2xl:gap-4">
        <Card
          icon={Rocket}
          title="Zero-downtime deploys"
          text="Every deploy **starts next to the live one** and only gets traffic once its health check passes."
          href={links.deploys}
          className="sm:col-span-2 xl:col-span-6"
        >
          <ul className="absolute bottom-6 left-6 z-10 space-y-1.5 text-sm text-foreground-light">
            {['Health checks before traffic', 'One-click rollbacks', 'Crashed containers replaced'].map((item) => (
              <li key={item} className="flex items-center gap-2">
                <Check className="size-4 text-foreground-lighter" aria-hidden="true" />
                {item}
              </li>
            ))}
          </ul>
          <div className="absolute top-1/2 right-4 size-56 -translate-y-1/2 max-sm:top-auto max-sm:right-auto max-sm:bottom-20 max-sm:left-1/2 max-sm:size-40 max-sm:-translate-x-1/2 max-sm:translate-y-0 sm:right-8 sm:size-64 md:right-12">
            <ArtFerry />
          </div>
        </Card>
        <Card
          icon={Boxes}
          title="Services"
          text="Run **web services, private services, workers, cron jobs** and static sites."
          href={links.services}
          className="xl:col-span-3"
        >
          <ArtServices />
        </Card>
        <Card
          icon={Database}
          title="Postgres & Redis"
          text="**Managed databases** on persistent volumes, wired into your services by reference."
          href={links.datastores}
          className="xl:col-span-3"
        >
          <ArtDatastores />
        </Card>
        <Card
          icon={Globe}
          title="Domains & HTTPS"
          text="A URL for every service. **Custom domains get HTTPS** from Let’s Encrypt."
          href={links.networking}
          className="xl:col-span-3"
        >
          <ArtDomains />
        </Card>
        <Card
          icon={CalendarClock}
          title="Cron jobs"
          text="**Scheduled jobs** on a cron expression, plus one-off runs such as migrations."
          href={links.jobs}
          className="xl:col-span-3"
        >
          <ArtCron />
        </Card>
        <Card
          icon={ScrollText}
          title="Logs & metrics"
          text="**Live build and runtime logs**, with CPU and memory for every service."
          href={links.logs}
          className="xl:col-span-3"
        >
          <ArtLogs />
        </Card>
        <Card
          icon={FileCode2}
          title="Blueprints"
          text="**Describe the whole stack in one file.** Render’s render.yaml works as it is."
          href={links.blueprint}
          className="xl:col-span-3"
        >
          <ArtBlueprint />
        </Card>
      </div>

      <p className="mt-14 max-w-[46rem] font-display text-xl leading-snug font-medium text-foreground-lighter sm:text-2xl">
        <span className="text-foreground">One binary on one machine.</span> The API, the dashboard, the builder, the
        proxy and the scheduler all run inside ferryd, and Docker is the only dependency.
      </p>
    </section>
  )
}

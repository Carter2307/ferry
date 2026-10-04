import { siGithub } from 'simple-icons'

import { githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'
import { Reveal } from './Reveal'

export function Hero() {
  return (
    <section aria-labelledby="hero-title" className="container-page pt-16 pb-12 sm:pt-24 lg:pt-28 lg:pb-14">
      <div className="grid gap-6 lg:grid-cols-2 lg:gap-12">
        <div>
          <h1 id="hero-title" className="heading-hero">
            <Reveal as="span" immediate className="block">
              Your own Render,
            </Reveal>
            <Reveal as="span" immediate delay={0.08} className="block text-primary">
              on one machine
            </Reveal>
          </h1>
          <Reveal immediate delay={0.24} className="mt-8 flex flex-wrap gap-3 max-lg:hidden">
            <HeroActions />
          </Reveal>
        </div>
        <div className="lg:pt-1.5">
          <Reveal immediate delay={0.16}>
            <p className="max-w-[31rem] text-foreground-light sm:text-lg lg:text-base">
              Deploy from a git repo, a folder or a Docker image, with zero downtime. Add Postgres, Redis, cron jobs,
              custom domains and HTTPS, all on one server with Docker.
            </p>
          </Reveal>
          <Reveal immediate delay={0.24} className="mt-8 flex flex-wrap gap-3 lg:hidden">
            <HeroActions />
          </Reveal>
        </div>
      </div>
    </section>
  )
}

export function HeroActions() {
  return (
    <>
      <a className="btn btn-primary" href={links.quickstart}>
        Get started
      </a>
      <a className="btn btn-secondary" href={githubUrl}>
        <BrandIcon icon={siGithub} />
        View on GitHub
      </a>
    </>
  )
}

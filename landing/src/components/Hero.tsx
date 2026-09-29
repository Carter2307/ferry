import { siGithub } from 'simple-icons'

import { githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'

export function Hero() {
  return (
    <section aria-labelledby="hero-title" className="container-page pt-16 pb-12 sm:pt-24 lg:pt-32 lg:pb-16">
      <div className="grid gap-6 lg:grid-cols-2 lg:gap-12">
        <div>
          <h1 id="hero-title" className="heading-hero">
            <span className="block">Your own Render,</span>
            <span className="block text-primary">on one machine</span>
          </h1>
          <div className="mt-8 flex flex-wrap gap-3 max-lg:hidden">
            <HeroActions />
          </div>
        </div>
        <div className="lg:pt-1.5">
          <p className="max-w-[31rem] text-foreground-lighter sm:text-lg lg:text-base">
            Deploy from a git repo, a folder or a Docker image, with zero downtime. Add Postgres, Redis, cron jobs,
            custom domains and HTTPS, all on one server with Docker.
          </p>
          <div className="mt-8 flex flex-wrap gap-3 lg:hidden">
            <HeroActions />
          </div>
        </div>
      </div>
    </section>
  )
}

function HeroActions() {
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

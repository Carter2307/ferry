import { siGithub } from 'simple-icons'

import { githubUrl, links } from '@/config'

import { BrandIcon } from './BrandIcon'

export function FinalCta() {
  return (
    <section aria-labelledby="cta-title" className="border-t border-border">
      <div className="container-page flex flex-col items-center py-24 text-center lg:py-32">
        <h2 id="cta-title" className="heading-section">
          <span className="text-foreground-lighter">Your own Render,</span> on one machine
        </h2>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <a className="btn btn-primary" href={links.quickstart}>
            Get started
          </a>
          <a className="btn btn-secondary" href={githubUrl}>
            <BrandIcon icon={siGithub} />
            View on GitHub
          </a>
        </div>
      </div>
    </section>
  )
}

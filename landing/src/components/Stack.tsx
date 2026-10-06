import {
  siDocker,
  siGithub,
  siGitlab,
  siGo,
  siLetsencrypt,
  siNginx,
  siNodedotjs,
  siPostgresql,
  siPython,
  siRedis,
  siRuby,
  siRust,
} from 'simple-icons'

import { Divider } from './backdrop'
import { BrandIcon } from './BrandIcon'
import { Reveal } from './Reveal'

/** Runtimes Ferry detects, then what it runs around them. */
const ITEMS = [
  { icon: siNodedotjs, name: 'Node.js' },
  { icon: siPython, name: 'Python' },
  { icon: siGo, name: 'Go' },
  { icon: siRust, name: 'Rust' },
  { icon: siRuby, name: 'Ruby' },
  { icon: siDocker, name: 'Dockerfile' },
  { icon: siPostgresql, name: 'Postgres' },
  { icon: siRedis, name: 'Redis' },
  { icon: siGithub, name: 'GitHub' },
  { icon: siGitlab, name: 'GitLab' },
  { icon: siLetsencrypt, name: 'Let’s Encrypt' },
  { icon: siNginx, name: 'nginx' },
]

export function Stack() {
  return (
    <section aria-labelledby="stack-title">
      <div className="container-page">
        <Reveal>
          <h2 id="stack-title" className="pb-6 font-sans text-sm font-normal text-foreground-lighter">
            Builds and runs the stack you already have
          </h2>
        </Reveal>
      </div>
      {/* The next section's top line closes the band. */}
      <div>
        <Divider delay="-7s" reverse />
        <Reveal className="container-page">
          <ul className="grid grid-cols-2 border-x border-border sm:grid-cols-3 lg:grid-cols-6">
            {ITEMS.map((item) => (
              <li
                key={item.name}
                className="flex items-center justify-center gap-2.5 px-3 py-7 text-foreground-lighter transition-colors hover:text-foreground"
              >
                <BrandIcon icon={item.icon} className="size-5 shrink-0" />
                <span className="text-[15px] font-medium">{item.name}</span>
              </li>
            ))}
          </ul>
        </Reveal>
      </div>
    </section>
  )
}

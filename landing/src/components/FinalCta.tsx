import { Divider } from './backdrop'
import { HeroActions } from './Hero'
import { Reveal } from './Reveal'

export function FinalCta() {
  return (
    <section aria-labelledby="cta-title">
      <Divider delay="-11s" reverse />
      <Reveal className="container-page flex flex-col items-center py-24 text-center lg:py-32">
        <h2 id="cta-title" className="heading-section">
          <span className="text-foreground-lighter">Your own Render,</span> on one machine
        </h2>
        <p className="mt-4 max-w-[30rem] text-foreground-lighter">
          Start the server, run <code className="inline-code">ferry up</code> in a folder and open the URL it gives
          you.
        </p>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <HeroActions />
        </div>
      </Reveal>
    </section>
  )
}

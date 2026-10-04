import { domAnimation, LazyMotion, MotionConfig } from 'motion/react'

import { HeroBackdrop, PageEndBackdrop, PageRails } from './components/backdrop'
import { Features } from './components/Features'
import { FinalCta } from './components/FinalCta'
import { Footer } from './components/Footer'
import { Hero } from './components/Hero'
import { Install } from './components/Install'
import { Nav } from './components/Nav'
import { OpenSource } from './components/OpenSource'
import { Showcase } from './components/Showcase'
import { Sources } from './components/Sources'
import { Stack } from './components/Stack'

export function App() {
  return (
    // Only the animation features of Motion that `Reveal` needs. A visitor who asks for reduced
    // motion gets the fades with no movement.
    <LazyMotion features={domAnimation} strict>
      <MotionConfig reducedMotion="user">
        <div className="relative isolate overflow-x-clip">
          <PageRails />
          <HeroBackdrop />
          <PageEndBackdrop />
          <a
            href="#main"
            className="fixed top-3 left-3 z-50 -translate-y-20 rounded-md bg-foreground px-3 py-2 text-sm text-background focus:translate-y-0"
          >
            Skip to content
          </a>
          <Nav />
          <main id="main">
            <Hero />
            <Showcase />
            <Stack />
            <Features />
            <Sources />
            <Install />
            <OpenSource />
            <FinalCta />
          </main>
          <Footer />
        </div>
      </MotionConfig>
    </LazyMotion>
  )
}

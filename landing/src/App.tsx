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
    <>
      <a
        href="#main"
        className="fixed top-3 left-3 z-50 -translate-y-20 rounded-md bg-foreground px-3 py-2 text-sm text-background focus:translate-y-0"
      >
        Skip to content
      </a>
      <Nav />
      <main id="main">
        <Hero />
        <Features />
        <Stack />
        <Showcase />
        <Sources />
        <OpenSource />
        <Install />
        <FinalCta />
      </main>
      <Footer />
    </>
  )
}

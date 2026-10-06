/*
 * The harbor figure of the deploy demo: a Hairline figure, made with the `hairline-create` skill
 * (https://hairline.lucasmarkes.com/skill).
 *
 *   slip.js                       the figure, in the format of the skill: do not convert it to
 *                                 TypeScript, the skill's tools check and build this file as it is
 *   public/hairline-kernel.js     the engine the figure draws with: the core of
 *                                 @lucasmarkes/hairline (MIT), as the skill gives it, unchanged
 *
 * The figure is written for the skill's bench, a page where the kernel is a script that defines
 * the global `HL` and where the figure calls the global `hairline(...)` to declare itself. This
 * module is that bench for the landing page: it loads the kernel, takes the declaration, and
 * mounts the figure on an element.
 */

/** The running figure. `seek` and `rate` are the two calls slip.js adds for a host that owns the clock. */
export interface SlipHandle {
  /** The rate of the clock while the pointer is on the figure, from 1 (no change) down to 0. */
  set(value: number): void
  /** Draws the handover at `ms` of the deploy (the clock of deploy/timeline.ts). */
  seek(ms: number): void
  /** How fast the clock must run now: 1, or less while the pointer is on the figure. */
  rate(): number
  destroy(): void
}

interface Figure {
  name: string
  /** One sentence that says what the figure shows. */
  means: string
  mount(host: { stage: HTMLElement; svg: SVGSVGElement; read: { textContent: string | null } }, value: number): SlipHandle
}

declare global {
  interface Window {
    /** The kernel: only what a host needs from it. */
    HL?: {
      inject(root: Document): void
      mk(tag: 'svg', attributes: Record<string, string>, parent: Element): SVGSVGElement
    }
    hairline?: (figure: Figure) => void
  }
}

let figure: Promise<Figure> | undefined

function load(): Promise<Figure> {
  figure ??= new Promise<Figure>((resolve, reject) => {
    window.hairline = resolve
    const kernel = document.createElement('script')
    kernel.src = `${import.meta.env.BASE_URL}hairline-kernel.js`
    kernel.onload = () => {
      import('./slip.js').catch(reject)
    }
    kernel.onerror = () => reject(new Error('The Hairline kernel did not load.'))
    document.head.append(kernel)
  })
  return figure
}

/**
 * Draws the figure in `stage` (an element with the attribute `data-hairline`, which the kernel's
 * stylesheet styles). `slow` is the rate of the clock while the pointer is on the figure.
 * The promise fails when the kernel or the figure did not load.
 */
export async function mountSlip(stage: HTMLElement, slow: number): Promise<SlipHandle & { means: string }> {
  const { mount, means } = await load()
  const kernel = window.HL
  if (!kernel) throw new Error('The Hairline kernel did not load.')
  kernel.inject(document)
  const svg = kernel.mk('svg', { viewBox: '0 0 400 320', 'aria-hidden': 'true' }, stage)
  // The figure writes the name of the stage under the pointer here. The page has its own caption.
  const handle = mount({ stage, svg, read: { textContent: null } }, slow)
  return {
    ...handle,
    means,
    destroy() {
      handle.destroy()
      svg.remove()
    },
  }
}

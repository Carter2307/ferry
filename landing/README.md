# Ferry landing page

The marketing page for Ferry: React 19, TypeScript, Tailwind CSS v4 and Vite, built to a static site. It lives in its own folder with its own toolchain, like the dashboard in `../web` and the docs in `../docs`. Its layout and style follow supabase.com, the same way the dashboard follows Supabase Studio. Its background and its motion (the pixel fields, the beams on the hairlines, content that appears on scroll) come from the [ferry-ui](https://github.com/Carter2307/ferry-ui) docs site, and the pixel fields are drawn by [ferry-shaders](https://www.npmjs.com/package/ferry-shaders).

```bash
cd landing
npm install
npm run dev          # → http://localhost:5175
npm run build        # → dist/ (typecheck, then a static build)
npm run preview      # serves dist/ on http://localhost:4175
```

| Setting | Default | Used for |
|---|---|---|
| `VITE_DOCS_URL` | `http://localhost:3000` | where the docs site is served (links go to `$VITE_DOCS_URL/docs/…/`); set it at build time for a public site |

The page imports the Ferry design tokens from `../web/src/styles/tokens.css` (colors for light and dark, radii, fonts), like the docs do. Build it from a full checkout of the repository, not from `landing/` alone.

## Layout

```
src/
  App.tsx                    page order, the backdrops behind the page
  config.ts                  docs and GitHub links
  theme.ts                   light/dark: follows the system until the visitor picks one (header and footer toggle)
  index.css                  tokens → Tailwind theme, headings, buttons, cards, masks, beams
  components/
    Nav.tsx  Hero.tsx        transparent sticky header; headline, paragraph and actions
    Showcase.tsx             under the headline, a tabbed window: the `ferry up` demo, the dashboard, a blueprint
      deploy/                the demo: script (timeline.ts), clock, terminal, harbor figure
        hairline/            the figure (slip.js) and the module that mounts it
      DashboardMock.tsx  BlueprintFile.tsx
    Stack.tsx                runtimes and tools (simple-icons)
    Features.tsx  art.tsx    the product grid and its card illustrations
    Sources.tsx              "Deploy from…": git, folder or image, with real CLI output
    Install.tsx  OpenSource.tsx  FinalCta.tsx  Footer.tsx
    backdrop.tsx             the two pixel fields of the page (ferry-shaders), the rails, the divider between sections
    Reveal.tsx               content that appears with a fade and a small rise (Motion)
    ThemeToggle.tsx  ui.tsx  the theme switch; section, heading and emphasis helpers
public/theme-init.js         applies the theme before the first paint
public/hairline-kernel.js    the engine of the harbor figure (see below), with its license
```

## The deploy demo

One `ferry up --follow` run in the Showcase's first tab. The terminal prints Ferry's real output (from `crates/ferry-cli` and `crates/ferry-engine`), and the harbor figure shows the same moment: a new ferry docks next to the live one, its health check passes, its ramp runs out and the cars (the requests) go to it in one step, and the old ferry leaves.

- It starts when the window is on screen (on most screens, as soon as the page loads), pauses while another tab is open, and has Pause/Replay.
- The pointer on the harbor slows the clock to a quarter, for the terminal too, so the handover can be read.
- The script is in `deploy/timeline.ts`: `LINES` for the terminal, `SCENE` for the captions of the figure. The figure has its own copy of the moments (the constants at the top of `hairline/slip.js`). Keep the three in step, and update the lines if ferry's output changes.
- In dev, `?t=5000` freezes the demo 5 s in, which helps when working on one frame.
- With `prefers-reduced-motion`, it shows the final frame and the full log.

### The harbor figure

The harbor is a [Hairline](https://hairline.lucasmarkes.com) figure: an isometric line drawing in one stroke, made with the `hairline-create` skill (`npx skills add lucasmarkes/hairline`, or see https://hairline.lucasmarkes.com/skill).

| File | What it is |
|---|---|
| `src/components/deploy/hairline/slip.js` | The figure, in the format of the skill: plain JavaScript that takes the engine from the global `HL` and declares itself with `hairline({ ... })`. Do not convert it to TypeScript and do not import anything in it: the skill's tools read this file as it is. |
| `public/hairline-kernel.js` | The engine: the core of `@lucasmarkes/hairline` (MIT, `hairline-kernel.LICENSE.txt`), as the skill gives it. Never edit it: the skill checks its hash. The npm package has only its own nineteen figures and does not export the engine, so the page loads this file. |
| `src/components/deploy/hairline/index.ts` | The page's side: loads the kernel, takes the figure's declaration and mounts it on an element. |
| `src/components/deploy/HarborFigure.tsx` | The React component: gives the demo's clock to the figure (`seek`), takes the hovered rate back (`rate`), and shows the captions. |

To change the drawing, edit `slip.js`, then check it with the skill, from any folder (it writes `hairline-slip.html`, the figure alone on a page, and `hairline-slip-look.png`, eight pictures of it):

```bash
node ~/.claude/skills/hairline-create/look.mjs landing/src/components/deploy/hairline/slip.js \
  --answer 74,38,7 --edge 18,75,0 --edge 96,-24,0
```

It must exit 0. The ten rules of a figure are in the skill's `rules.md`: no color of its own (the palette is the `--hairline-*` variables, which `.harbor-figure` in `index.css` sets from Ferry's tokens), no text in the drawing, every motion read off the one clock.

## Background and motion

- The pixel field is the `PixelField` shader of ferry-shaders (WebGPU): a dense field of 3px pixels that thins out into the page. The page has two (`backdrop.tsx`): one from the very top (behind the transparent header and the headline) and one from the very bottom (behind the footer, which has no background and ends with a free band). They take their colors from the tokens (`foreground`, `primary-bright`), share one GPU device and stop when they are out of view. A browser with no WebGPU gets a still CSS pattern (`.pixel-fallback`).
- `PageRails` draws two hairlines at the edges of the page column on wide screens. `Divider` is the hairline between two sections. Short beams of light run along them (CSS animations in `index.css`).
- `Reveal` makes content appear with a fade and a small rise: at once at the top of the page, on scroll below. Stagger siblings with `delay`.
- With `prefers-reduced-motion`, the beams do not run, the pixel fields show one still frame and content appears with a fade only.
- Three libraries do this work: `ferry-shaders` (the pixel fields), `motion` (the fades; only its `m` components and `domAnimation`), and the Hairline kernel (the harbor figure, loaded when the demo mounts).

## Design rules

- Colors come from the tokens: neutral canvas and hairline borders, the blue primary for actions and "live" routes, green for healthy/live states. The light canvas is pure white.
- Type: Manrope for headings (a full line and a quieter one), Inter for text, Source Code Pro only for code.
- Buttons and tabs have 8px corners; badges 4px. Nothing is pill-shaped, and there are no monospace or uppercase labels.
- Illustrations are UI fragments and line art in the greys, fading into their card. The harbor figure is line art too: greys, and the blue stroke for the live deploy only.

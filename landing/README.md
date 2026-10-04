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
      deploy/                the demo: script (timeline.ts), clock, terminal, harbor drawing
      DashboardMock.tsx  BlueprintFile.tsx
    Stack.tsx                runtimes and tools (simple-icons)
    Features.tsx  art.tsx    the product grid and its card illustrations
    Sources.tsx              "Deploy from…": git, folder or image, with real CLI output
    Install.tsx  OpenSource.tsx  FinalCta.tsx  Footer.tsx
    backdrop.tsx             the two pixel fields of the page (ferry-shaders), the rails, the divider between sections
    Reveal.tsx               content that appears with a fade and a small rise (Motion)
    ThemeToggle.tsx  ui.tsx  the theme switch; section, heading and emphasis helpers
public/theme-init.js         applies the theme before the first paint
```

## The deploy demo

One `ferry up --follow` run in the Showcase's first tab. The terminal prints Ferry's real output (from `crates/ferry-cli` and `crates/ferry-engine`), and the harbor drawing shows the same moment: the new deploy docks next to the live one, its health check passes, the proxy switches requests to it in one step, and the old deploy finishes its requests and leaves.

- It starts when the window is on screen (on most screens, as soon as the page loads), pauses while another tab is open, and has Pause/Replay.
- Change the script in `deploy/timeline.ts`: `LINES` for the terminal, `SCENE` for the drawing. Keep them in step, and update the lines if ferry's output changes.
- In dev, `?t=5000` freezes the demo 5 s in, which helps when working on one frame.
- With `prefers-reduced-motion`, it shows the final frame and the full log.

## Background and motion

- The pixel field is the `PixelField` shader of ferry-shaders (WebGPU): a dense field of 3px pixels that thins out into the page. The page has two (`backdrop.tsx`): one from the very top (behind the transparent header and the headline) and one from the very bottom (behind the footer, which has no background and ends with a free band). They take their colors from the tokens (`foreground`, `primary-bright`), share one GPU device and stop when they are out of view. A browser with no WebGPU gets a still CSS pattern (`.pixel-fallback`).
- `PageRails` draws two hairlines at the edges of the page column on wide screens. `Divider` is the hairline between two sections. Short beams of light run along them (CSS animations in `index.css`).
- `Reveal` makes content appear with a fade and a small rise: at once at the top of the page, on scroll below. Stagger siblings with `delay`.
- With `prefers-reduced-motion`, the beams do not run, the pixel fields show one still frame and content appears with a fade only.

## Design rules

- Colors come from the tokens: neutral canvas and hairline borders, the blue primary for actions and "live" routes, green for healthy/live states. The light canvas is pure white.
- Type: Manrope for headings (a full line and a quieter one), Inter for text, Source Code Pro only for code.
- Buttons and tabs have 8px corners; badges 4px. Nothing is pill-shaped, and there are no monospace or uppercase labels.
- Illustrations are UI fragments and line art in the greys, fading into their card.

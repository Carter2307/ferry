# Ferry landing page

The marketing page for Ferry: React 19, TypeScript, Tailwind CSS v4 and Vite, built to a static site. It lives in its own folder with its own toolchain, like the dashboard in `../web` and the docs in `../docs`. Its layout and style follow supabase.com, the same way the dashboard follows Supabase Studio.

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
  App.tsx                    page order
  config.ts                  docs and GitHub links
  theme.ts                   light/dark: follows the system until the visitor picks one (footer toggle)
  index.css                  tokens → Tailwind theme, headings, buttons, cards, masks
  components/
    Nav.tsx  Hero.tsx        sticky header; headline, paragraph and actions
    Features.tsx  art.tsx    the product grid and its card illustrations
    Stack.tsx                runtimes and tools (simple-icons)
    Showcase.tsx             tabbed window: the `ferry up` demo, the dashboard, a blueprint
      deploy/                the demo: script (timeline.ts), clock, terminal, harbor drawing
      DashboardMock.tsx  BlueprintFile.tsx
    Sources.tsx              "Deploy from…": git, folder or image, with real CLI output
    OpenSource.tsx  Install.tsx  FinalCta.tsx  Footer.tsx
public/theme-init.js         applies the theme before the first paint
```

## The deploy demo

One `ferry up --follow` run in the Showcase's first tab. The terminal prints Ferry's real output (from `crates/ferry-cli` and `crates/ferry-engine`), and the harbor drawing shows the same moment: the new deploy docks next to the live one, its health check passes, the proxy switches requests to it in one step, and the old deploy finishes its requests and leaves.

- It starts when the window scrolls into view, pauses while another tab is open, and has Pause/Replay.
- Change the script in `deploy/timeline.ts`: `LINES` for the terminal, `SCENE` for the drawing. Keep them in step, and update the lines if ferry's output changes.
- In dev, `?t=5000` freezes the demo 5 s in, which helps when working on one frame.
- With `prefers-reduced-motion`, it shows the final frame and the full log.

## Design rules

- Colors come from the tokens: neutral canvas and hairline borders, the blue primary for actions and "live" routes, green for healthy/live states. The light canvas is pure white.
- Type: Manrope for headings (a full line and a quieter one), Inter for text, Source Code Pro only for code.
- Buttons and tabs have 8px corners; badges 4px. Nothing is pill-shaped, and there are no monospace or uppercase labels.
- Illustrations are UI fragments and line art in the greys, fading into their card.

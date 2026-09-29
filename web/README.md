# Ferry web dashboard

The Ferry dashboard is a single-page app in React 19, TypeScript (strict), Vite, Tailwind CSS v4 and shadcn/ui, with a Supabase Studio look. It lives in its own folder with its own toolchain. The Rust workspace never runs Node: `ferry-api` only embeds the already-built `web/dist`.

| | |
|---|---|
| UI | React 19, shadcn/ui (Radix primitives, customized in `src/components/ui`), lucide icons |
| Styling | Tailwind CSS v4 (`@tailwindcss/vite`), Supabase tokens in `src/styles/globals.css`, light + dark |
| Server state | TanStack Query v5 (`src/lib/api/queries/*`) |
| Client state | Zustand (`src/stores/auth.ts`, `src/stores/ui.ts`) |
| Routing | React Router v7 (`createBrowserRouter`, history URLs, code-split pages) |
| Tests | Vitest (`src/**/*.test.{ts,tsx}`; components render with `react-dom/server`) |

## Develop

```sh
cd web
npm install
# point the dev server at a running ferryd (default http://127.0.0.1:7878)
FERRY_API_URL=http://127.0.0.1:7878 npm run dev     # → http://localhost:5173
```

Vite proxies `/api`, `/hooks` and `/healthz` to `FERRY_API_URL`. Server-Sent Events (log streams and the change feed) pass through unbuffered. Sign in with the API token that `ferryd` prints on first start (it is also stored in `<data-dir>/api_token`).

| Script | What it does |
|---|---|
| `npm run dev` | Vite dev server with HMR and the API proxy |
| `npm run build` | `tsc -b && vite build` → `dist/` (hashed files under `dist/assets/`) |
| `npm run preview` | serves `dist/` with the same proxy |
| `npm test` | Vitest unit tests (SSE parser, formatting, `.env` parsing, resource limits, a few components) |
| `npm run typecheck` | `tsc -b --noEmit` |
| `npm run lint` | ESLint (typescript-eslint, react-hooks, react-refresh) |

## How ferryd serves it

1. Run `npm run build` in `web/`. It produces `web/dist/index.html` and `web/dist/assets/*` (hashed names).
2. `ferry-api` embeds `web/dist` at compile time through its build script. The Rust build never invokes npm, so build the web app first.
3. `ferryd` serves the SPA at `/`:
   - Every non-API `GET` with no matching file falls back to `index.html`, so history URLs such as `/services/api/logs` work on reload.
   - Files under `/assets/*` are sent with `Cache-Control: public, max-age=31536000, immutable`.
   - `index.html` is sent with `Cache-Control: no-cache`.

The app always calls the API on the same origin (`/api/v1/...`), so no CORS setup is needed.

## Transport: why HTTP + SSE (no gRPC, no WebSocket)

- **Queries and mutations use HTTP(S) + JSON REST** (`/api/v1/*`, DESIGN.md §10). It maps directly onto TanStack Query (caching, dedupe, retries, invalidation) and matches what the CLI uses. The token travels in `Authorization: Bearer …`.
- **Server-to-client push uses Server-Sent Events**, read with `fetch()` and a streaming parser (`src/lib/api/sse.ts`), so the token stays in a header and never in a URL:
  - log streams: `/api/v1/deploys/{id}/logs`, `/api/v1/jobs/{id}/logs`, `/api/v1/services/{id}/logs` (`event: log` … `event: end`);
  - change feed: `/api/v1/events` (`event: ready`, then `event: change` with `{kind, id, service_id, action}`). Each change turns into TanStack Query invalidations (`src/lib/api/events.ts`). The client reconnects with exponential backoff from 1s to 15s. If the server has no change feed (404), queries fall back to polling: 5s for lists, 2–3s for the detail on screen.
- **Why not gRPC:** browsers would need gRPC-Web, an extra proxy and a protobuf toolchain, with no benefit for small JSON payloads.
- **Why not WebSocket:** the traffic is request/response plus one-way push. SSE already provides automatic reconnection, HTTP/2 multiplexing and header auth. A WebSocket only makes sense for a future interactive `exec` terminal.

## Layout

```
src/
  main.tsx  App.tsx (providers)  routes.tsx  RequireAuth.tsx
  styles/globals.css             Supabase tokens → shadcn variables (light + .dark), mono-label utility
  lib/api/client.ts              fetch wrapper, ApiError {status, code, message}
  lib/api/types.ts               TS mirror of ferry-core dto.rs / models.rs / logs.rs
  lib/api/endpoints.ts           one function per endpoint (+ SSE paths)
  lib/api/keys.ts                query-key factory
  lib/api/sse.ts                 fetch-based SSE reader
  lib/api/events.ts              change feed → invalidations, polling fallback
  lib/api/queries/*.ts           query + mutation hooks per resource
  lib/api/useLogStream.ts        log stream hook
  lib/format.ts  lib/dotenv.ts   pure helpers (tested)
  lib/resources.ts               memory / CPU limits: parse + format (mirror of ferry-core resources.rs), presets, form model
  stores/auth.ts  stores/ui.ts   zustand
  components/ui/*                shadcn primitives (customized)
  components/shell/*             AppShell, TopBar, IconRail, InnerMenu, CommandMenu, ConnectPopover
  components/patterns/*          PageHeader, FormCard, InfoTile, MetricCard, EmptyState, LogViewer, …
  pages/**                       routed pages (each file exports a named `XxxPage`)
```

The token is kept in `localStorage` under `ferry.token`, the same key the previous single-file dashboard used, so existing sessions carry over. UI preferences (theme, rail, log viewer options) are kept under `ferry.ui`. `public/theme-init.js` applies the saved theme before first paint. It is an external file so the app works under a strict `script-src 'self'` Content Security Policy.

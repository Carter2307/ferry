# Ferry docs

The documentation site of Ferry: [Fumadocs](https://fumadocs.dev) on Next.js 16 (App Router, React 19, TypeScript strict, Tailwind CSS v4), exported as a static site. It lives in its own folder with its own toolchain, like the web dashboard in `../web`.

| | |
|---|---|
| Content | MDX in `content/docs/` (one `meta.json` per folder for titles, order and icons) |
| Design | the Ferry design tokens from `../web/src/styles/tokens.css` (shared with the dashboard), mapped onto Fumadocs in `app/global.css`; Inter + Source Code Pro |
| Layout | Fumadocs "notebook" layout with a custom 48px header (`components/layout/site-header.tsx`) and sidebar (`components/layout/sidebar.tsx`) |
| Generated references | CLI, server options and the API reference, from the Rust binaries (`scripts/generate.mjs`) |
| Search | Fumadocs built-in (static Orama index at `/api/search`, works in the export) |
| Output | `out/`, deployable on any static host — including Ferry itself |

## Develop

Requirements: Node.js 20.19+ (npm). Rust is only needed to regenerate the reference pages.

```bash
cd docs
npm install
npm run dev          # → http://localhost:3000
```

| Script | What it does |
|---|---|
| `npm run dev` | Next.js dev server with hot reload |
| `npm run build` | static export to `out/`, then the link check |
| `npm start` | serves `out/` locally (`serve`) |
| `npm run typecheck` | `next typegen && tsc --noEmit` |
| `npm run generate` | regenerates the reference pages from the Rust code (needs `cargo`) |
| `npm run generate:check` | fails when a generated file is stale (for CI) |
| `npm run check-links` | checks `out/` for broken internal links and anchors |

## Write content

Pages are MDX files in `content/docs/`. The URL follows the path: `content/docs/concepts/deploys.mdx` → `/docs/concepts/deploys/`.

```mdx
---
title: Deploys
description: How a deploy goes from queued to live with zero downtime.
---

Body in Markdown, with the components below.
```

- **Frontmatter:** `title` (required), `description` (one sentence, shown under the title and in search/Open Graph; plain text, no Markdown), optional `icon` (a [lucide](https://lucide.dev/icons) name) and `full: true` for a full-width page. Don't repeat the title as a `#` heading: the page renders it.
- **Order and sidebar:** each folder has a `meta.json` (`title`, `icon`, `pages` in order). A top-level folder is a sidebar section with an UPPERCASE label; a page missing from `pages` is still built but not listed. Nested folders are collapsible groups; a folder's `index.mdx` is its link.
- **Links:** absolute URLs without a trailing slash (`/docs/concepts/deploys`, `/docs/reference/cli#ferry-deploy`) or relative file paths (`./deploys.mdx`). `npm run build` fails on a broken link or anchor.
- **Headings:** `##` and `###` build the "On this page" table of contents; anchors are the slugified heading text (`## Health checks` → `#health-checks`).
- **Code blocks:** fenced with a language; meta options `title="Terminal"`, `lineNumbers`, `noCopy`. Mark lines with a trailing comment: `# [!code highlight]`, `# [!code ++]` / `# [!code --]`, `# [!code focus]` (use the language's comment syntax).
- **Components** (available everywhere, no import needed):

| Component | Props | Use |
|---|---|---|
| `<Callout>` | `type`: `info` (default), `tip`, `success`, `warning`, `danger`; `title` | notes and warnings |
| `<Cards>` / `<Card>` | `Cards`: `cols` 2 (default) or 3 · `Card`: `title`, `description`, `href`, `icon`, `external` | link grids |
| `<Steps>` / `<Step>` | — (start each step with a `###` heading) | procedures |
| `<Tabs>` / `<Tab>` | `Tabs`: `items`, `groupId`, `persist`, `defaultIndex` · `Tab`: `value` (one of `items`) | CLI / API / dashboard variants |
| `<Accordions>` / `<Accordion>` | `Accordion`: `title` | FAQs |
| `<Files>` / `<Folder>` / `<File>` | `name`, `defaultOpen` | file trees |
| `<TypeTable>` | `type`: `{ field: { type, description, default, required } }` | option tables |
| `<Badge>` | `variant`: `neutral`, `primary`, `success`, `warning`, `danger` | inline pills |
| `<Mermaid>` | `chart` | diagrams (or a ` ```mermaid ` fence) |

  Icons for cards: `import { Rocket } from 'lucide-react';` at the top of the MDX file, then `icon={<Rocket />}`.

- **Diagrams:** write a ` ```mermaid ` code fence (flowchart, sequence, state…). It renders in the browser with the Ferry colors in light and dark mode. Prefer `flowchart TB` for anything with more than four nodes so it fits the 760px column. Hand-made SVGs go in `public/` and are referenced as `/file.svg`.

## Generated references

Never edit these by hand; `scripts/generate.mjs` writes them from the Rust code:

| File | Source |
|---|---|
| `openapi/ferry.json` | `cargo run -p ferryd -- --dump-openapi` (verbatim) |
| `content/docs/reference/api/*` | fumadocs-openapi, one page per OpenAPI tag, plus `index.mdx` and `meta.json` |
| `content/docs/reference/cli.mdx` | `cargo run -p ferry-cli -- markdown-help` (clap-markdown) |
| `content/docs/reference/server-options.mdx` | `cargo run -p ferryd -- --dump-markdown-help`, plus the environment variables from `ferryd --help` |

```bash
npm run generate        # after changing a command, a flag, a handler or a DTO
npm run generate:check  # CI: exits 1 when a committed file is stale
```

The generated files are committed, so `npm run build` works without Rust. To change their text, change the Rust doc comments (clap help, `#[utoipa::path]`, schemas) and regenerate.

The API pages have no "try it" playground: `ferryd` sends no CORS headers, so a browser can't call it from the docs' origin. Every server serves its own Swagger UI at `/api/docs` for that.

## Build

```bash
npm run build     # → out/ (static HTML, the search index, Open Graph images), then the link check
npm start         # preview out/ on http://localhost:3000
```

Build-time settings (environment variables):

| Variable | Default | Used for |
|---|---|---|
| `DOCS_SITE_URL` | `http://localhost:3000` | the public URL of the site: canonical and Open Graph URLs |
| `FERRY_DOCS_SERVER_URL` | `http://127.0.0.1:7878` | the "Dashboard" link and the server URL in the API request examples (set it for a public site: the default only works on the reader's machine) |
| `DOCS_GIT_BRANCH` | `dev` | the branch of the "Edit on GitHub" links; it must contain `docs/` |

URLs end with a slash (`/docs/concepts/deploys/` → `out/docs/concepts/deploys/index.html`), which every static server resolves. `out/404.html` is the not-found page.

## Deploy

`out/` is a plain static site: copy it to any static host. To host the docs on Ferry itself, as a static site served by nginx:

**From your machine** (upload the export):

```bash
cd docs
DOCS_SITE_URL=http://ferry-docs.localhost:8080 npm run build
ferry up ferry-docs --dir out --type static --follow
# → http://ferry-docs.localhost:8080
```

`out/` has an `index.html` at its root and no `package.json`, so Ferry serves it as-is. Run the same two commands to publish an update.

**From git** (Ferry builds the site, and redeploys on push with a GitHub webhook):

```bash
ferry create ferry-docs --type static \
  --repo https://github.com/Carter2307/ferry --branch dev \
  --build-cmd "cd docs && npm ci --include=dev && npm run build" \
  --publish-dir docs/out --follow
```

`docs/` is on the `dev` branch until it is merged into `main`. The build runs from the repository root because the docs import `../web/src/styles/tokens.css`. `--include=dev` is needed because Ferry runs build commands with `NODE_ENV=production`, which would skip the build tools. Add `-e DOCS_SITE_URL=https://docs.example.com` for the public URL (build-time env vars reach the build), and `--domain docs.example.com` for a custom domain.

## Layout

```
app/
  (home)/                 landing page (/) and its layout
  docs/                   docs layout (notebook, top nav) and [[...slug]] page
  not-found.tsx           404 page (exported as out/404.html)
  api/search/             static search index
  og/docs/                Open Graph images, one per page
  llms.txt, llms-full.txt, llms.mdx/   Markdown versions of the pages for LLMs
  global.css              Tailwind + Fumadocs presets, tokens → Fumadocs theme, prose tweaks
  icon.svg                favicon (Ferry mark)
components/
  layout/                 site header (theme menu), docs header slot, sidebar, site nav (drawer links)
  mdx/                    Callout, Card(s), Badge, Mermaid
  mdx.tsx                 the MDX component map
  api-page.tsx            OpenAPI page renderer (playground off, YAML/gzip bodies, bearer token in samples)
  search.tsx              search dialog (static index, pages ranked by title/heading match)
content/docs/             the pages
lib/                      source loader, OpenAPI loader, shared settings
openapi/ferry.json        GENERATED
scripts/                  generate.mjs, check-links.mjs
```

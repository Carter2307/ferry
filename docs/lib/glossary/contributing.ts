import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Rust and the repository ----------------------------------------------
  workspace: {
    term: 'workspace',
    definition: 'A set of Rust crates in one repository. `cargo` builds and tests them together.',
    see: ['crate', 'cargo'],
  },
  nodejs: {
    term: 'Node.js',
    definition: 'The program that runs JavaScript outside a browser. The builds of the dashboard and of the docs site need it.',
    see: ['npm'],
  },
  'debug-build': {
    term: 'debug build',
    definition: 'The default build of `cargo`. It compiles quickly and the programs go to `target/debug/`.',
    see: ['release-build'],
  },
  'release-build': {
    term: 'release build',
    definition: 'A build with full optimization. It compiles slower, and the programs run faster. They go to `target/release/`.',
    see: ['debug-build'],
  },
  merge: {
    term: 'merge',
    definition: 'To bring the commits of one branch into another branch.',
    see: ['branch', 'commit'],
  },
  dto: {
    term: 'DTO',
    definition: 'Data transfer object: a type that gives the form of the data in a request or in an answer of the API.',
  },
  formatter: {
    term: 'formatter',
    definition: 'A tool that writes the code in one standard layout. `cargo fmt` is the formatter of Rust.',
  },
  lint: {
    term: 'lint',
    definition: 'A check that finds common errors and bad style in the code. `cargo clippy` runs the lints of Rust.',
  },
  invariant: {
    term: 'invariant',
    definition: 'A condition that is always true at one point of the code.',
  },
  tracing: {
    term: 'tracing',
    definition: 'The Rust library that Ferry uses to write its log lines. Each line has a level, for example `info` or `debug`.',
  },
  'blocking-work': {
    term: 'blocking work',
    definition: 'Work that keeps a thread busy for a long time, for example heavy file work. Other tasks on that thread must wait.',
  },
  clap: {
    term: 'clap',
    definition: 'The Rust library that reads the commands and the flags of a program. It also writes the `--help` text.',
  },
  utoipa: {
    term: 'utoipa',
    definition: 'The Rust library that makes the OpenAPI document from annotations in the code.',
    see: ['openapi'],
  },
  'doc-comment': {
    term: 'doc comment',
    definition: 'A comment in Rust code that starts with `///`. Tools read it as the description of the item below it.',
  },

  // --- Tests ----------------------------------------------------------------
  'unit-test': {
    term: 'unit test',
    definition: 'A test of one small part of the code. It is in the same file as the code and needs no other program.',
  },
  'integration-test': {
    term: 'integration test',
    definition: 'A test in `crates/<crate>/tests/`. It uses the crate from the outside, as another crate does.',
  },
  'e2e-test': {
    term: 'end-to-end test',
    definition: 'A test that runs the real parts together, with a real Docker daemon.',
    see: ['gated-test'],
  },
  'gated-test': {
    term: 'gated test',
    definition: 'A test that runs only when a variable is set. Without the variable, it prints `skipped` and passes.',
  },
  mock: {
    term: 'mock',
    definition: 'A stand-in for a real part in a test. It records the calls that it gets and does no real work.',
  },
  fixture: {
    term: 'fixture',
    definition: 'Fixed data or a small app that a test uses as its input.',
  },
  'api-router': {
    term: 'router',
    definition: 'The part of the API that sends each request to the function for its path.',
  },
  'tls-handshake': {
    term: 'TLS handshake',
    definition: 'The start of a TLS connection. The server shows its certificate and the two sides agree on the keys.',
    see: ['tls'],
  },
  pebble: {
    term: 'Pebble',
    definition: 'A small ACME server for tests, from Let’s Encrypt. It gives certificates that browsers do not trust.',
    see: ['acme'],
  },
  'drop-guard': {
    term: 'drop guard',
    definition: 'A Rust value that runs its cleanup code when the function ends, also after a failure.',
  },
  'type-check': {
    term: 'type check',
    definition: 'A check that each value has the type that the code expects. It runs no code.',
  },

  // --- The web client -------------------------------------------------------
  spa: {
    term: 'single-page app',
    definition: 'A web app that loads one HTML page. JavaScript then changes the page, with no full reload.',
  },
  react: {
    term: 'React',
    definition: 'A JavaScript library that builds a web page from components.',
  },
  typescript: {
    term: 'TypeScript',
    definition: 'JavaScript with types. A compiler checks the types before the code runs.',
  },
  shadcn: {
    term: 'shadcn/ui',
    definition: 'A set of React components that you copy into your project and change.',
  },
  lucide: {
    term: 'lucide',
    definition: 'A set of icons. The dashboard and the docs site use it.',
  },
  tailwind: {
    term: 'Tailwind CSS',
    definition: 'A CSS tool. You style an element with short class names, for example `text-primary`.',
  },
  'tanstack-query': {
    term: 'TanStack Query',
    definition: 'A library that gets data from an API, keeps it in a cache and gets it again when it is stale.',
    see: ['invalidation'],
  },
  zustand: {
    term: 'Zustand',
    definition: 'A small library that keeps the state of the app in the browser.',
  },
  'react-router': {
    term: 'React Router',
    definition: 'A library that shows the correct page for each URL of a single-page app.',
    see: ['history-url'],
  },
  vite: {
    term: 'Vite',
    definition: 'The build tool of the dashboard. It serves the code in development and makes the files for production.',
  },
  vitest: {
    term: 'Vitest',
    definition: 'The test tool of the dashboard. It runs the files that end with `.test.ts` or `.test.tsx`.',
  },
  eslint: {
    term: 'ESLint',
    definition: 'The tool that runs the lints of JavaScript and TypeScript code.',
    see: ['lint'],
  },
  'dev-server': {
    term: 'dev server',
    definition: 'A local web server for development. When you save a file, it builds the code again and updates the page.',
  },
  hmr: {
    term: 'hot module replacement',
    definition: 'HMR: the dev server puts a changed file into the open page immediately. You do not reload the page.',
  },
  'history-url': {
    term: 'history URL',
    definition: 'A normal path, for example `/services/api/logs`, that the app reads in the browser. The server has no file for it.',
  },
  'code-splitting': {
    term: 'code splitting',
    definition: 'The build makes one file for each page. The browser loads the file only when it needs the page.',
  },
  'data-query': {
    term: 'query',
    definition: 'A request that reads data from the server. TanStack Query keeps the answer in a cache.',
    see: ['mutation'],
  },
  mutation: {
    term: 'mutation',
    definition: 'A request that changes data on the server.',
    see: ['data-query'],
  },
  invalidation: {
    term: 'invalidation',
    definition: 'A mark on data in the cache that says the data is stale. The library then gets the data again.',
  },
  polling: {
    term: 'polling',
    definition: 'To ask the server again at a fixed interval, to see if something changed.',
  },
  'local-storage': {
    term: 'localStorage',
    definition: 'A place in the browser where a page keeps small values. Each script of the page can read it.',
  },
  'http-only': {
    term: 'HttpOnly',
    definition: 'A flag of a cookie. The browser sends the cookie, but a script in the page cannot read it.',
    see: ['cookie'],
  },
  csp: {
    term: 'Content Security Policy',
    definition: 'CSP: a header that tells the browser which scripts and files a page can load.',
  },
  'cache-control': {
    term: 'Cache-Control',
    definition: 'A header that tells the browser how long it can keep a copy of a file.',
    see: ['etag'],
  },
  etag: {
    term: 'ETag',
    definition: 'A header that identifies one version of a file. The browser sends it back to ask if its copy is current.',
  },
  'design-token': {
    term: 'design token',
    definition: 'A value of the design that has a name, for example a color or a radius. Each project reads it from one file.',
  },
  'css-custom-property': {
    term: 'CSS custom property',
    definition: 'A CSS value with a name that starts with `--`. Other rules read it with `var()`.',
  },

  // --- The docs site --------------------------------------------------------
  fumadocs: {
    term: 'Fumadocs',
    definition: 'The framework of this docs site. It makes the pages, the sidebar and the search from MDX files.',
  },
  nextjs: {
    term: 'Next.js',
    definition: 'A React framework. It builds this docs site.',
  },
  mdx: {
    term: 'MDX',
    definition: 'Markdown that can also contain components, for example `<Diagram>`.',
  },
  frontmatter: {
    term: 'frontmatter',
    definition: 'The lines between `---` at the top of a page file. They give the title and the description.',
  },
  anchor: {
    term: 'anchor',
    definition: 'The part of a link after the `#`. It opens the page at one heading.',
  },
  mermaid: {
    term: 'Mermaid',
    definition: 'A text format for diagrams. The browser draws the diagram from the text.',
  },
  ste: {
    term: 'Simplified Technical English',
    definition: 'ASD-STE100: a standard with rules for short and clear technical sentences.',
  },
  bundler: {
    term: 'bundler',
    definition: 'The tool that puts the source files of a site together into the files that the browser loads.',
  },
} satisfies Record<string, GlossaryEntry>;

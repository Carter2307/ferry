/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Public URL of the docs site, without `/docs` (default http://localhost:3000). */
  readonly VITE_DOCS_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}

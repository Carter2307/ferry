import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createMDX } from 'fumadocs-mdx/next';

// The docs import the design tokens from ../web/src/styles/tokens.css, so the
// bundler root is the repository (Turbopack can't read files outside its root).
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  // Static export → out/ (any static host, including Ferry itself: see README.md).
  output: 'export',
  // /docs/foo/ → out/docs/foo/index.html, which every static server resolves.
  trailingSlash: true,
  images: { unoptimized: true },
  reactStrictMode: true,
  turbopack: { root: repoRoot },
  outputFileTracingRoot: repoRoot,
  // Build-time settings, inlined into server and client code.
  env: {
    DOCS_SITE_URL: process.env.DOCS_SITE_URL ?? 'http://localhost:3000',
    FERRY_DOCS_SERVER_URL: process.env.FERRY_DOCS_SERVER_URL ?? 'http://127.0.0.1:7878',
    DOCS_GIT_BRANCH: process.env.DOCS_GIT_BRANCH ?? 'dev',
  },
};

export default withMDX(config);

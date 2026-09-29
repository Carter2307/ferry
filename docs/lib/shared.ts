import { createGetUrl } from 'fumadocs-core/source';

export const appName = 'Ferry Docs';
export const appDescription =
  'Documentation for Ferry, a self-hosted Render alternative written in Rust: deploy web services, workers, cron jobs, static sites and managed Postgres/Redis on your own server.';

export const docsRoute = '/docs';
export const docsImageRoute = '/og/docs';
export const docsContentRoute = '/llms.mdx/docs';

/**
 * The repository the "Edit on GitHub" links point at. `branch` must be a branch
 * that contains docs/: set `DOCS_GIT_BRANCH` at build time (default `dev`, where
 * new work lands first).
 */
export const gitConfig = {
  user: 'Carter2307',
  repo: 'ferry',
  branch: process.env.DOCS_GIT_BRANCH ?? 'dev',
};

export const githubUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

/**
 * Public URL the static export is deployed at (canonical URLs, Open Graph images).
 * Set `DOCS_SITE_URL` at build time; see next.config.mjs.
 */
export const siteUrl = process.env.DOCS_SITE_URL ?? 'http://localhost:3000';

/**
 * The Ferry server the docs point at: the "Dashboard" link and the API reference's
 * request examples. `ferryd` serves the API and the dashboard on `--api-addr`
 * (default 127.0.0.1:7878). Set `FERRY_DOCS_SERVER_URL` at build time.
 */
export const ferryServerUrl = process.env.FERRY_DOCS_SERVER_URL ?? 'http://127.0.0.1:7878';

const getContentUrl = createGetUrl(docsContentRoute);

export function getPageMarkdownUrl(page: { slugs: string[]; locale?: string }) {
  const segments = [...page.slugs, 'content.md'];

  return { segments, url: getContentUrl(segments, page.locale) };
}

const getImageUrl = createGetUrl(docsImageRoute);

export function getPageImageUrl(page: { slugs: string[]; locale?: string }) {
  const segments = [...page.slugs, 'image.png'];

  return { segments, url: getImageUrl(segments, page.locale) };
}

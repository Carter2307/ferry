#!/usr/bin/env node
/**
 * Fails on broken internal links in the static export (out/): every
 * `<a href>` that points inside the site must reach an exported file, and a
 * `#fragment` must match an `id` on the target page.
 *
 * Runs after `next build` (see "build" in package.json), so it checks what is
 * actually deployed: MDX links, cards, the sidebar, the header and the TOC.
 *
 * Usage: node scripts/check-links.mjs [outDir]
 */
import { existsSync, statSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const DOCS_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OUT = path.resolve(DOCS_DIR, process.argv[2] ?? 'out');

if (!existsSync(OUT)) {
  console.error(`✖ ${path.relative(process.cwd(), OUT)} does not exist: run \`next build\` first.`);
  process.exit(1);
}

async function walk(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const abs = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await walk(abs)));
    else if (entry.name.endsWith('.html')) out.push(abs);
  }
  return out;
}

function decodeEntities(s) {
  return s
    .replace(/&amp;/g, '&')
    .replace(/&quot;/g, '"')
    .replace(/&#x27;|&#39;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>');
}

/** URL path of an exported HTML file: out/docs/foo/index.html → /docs/foo/ */
function urlOf(file) {
  const rel = path.relative(OUT, file).split(path.sep).join('/');
  if (rel === 'index.html') return '/';
  if (rel.endsWith('/index.html')) return `/${rel.slice(0, -'index.html'.length)}`;
  return `/${rel}`;
}

/** Exported file serving a URL path, or null. */
function fileFor(urlPath) {
  const clean = urlPath.replace(/^\/+/, '');
  const candidates = urlPath.endsWith('/')
    ? [path.join(OUT, clean, 'index.html')]
    : [path.join(OUT, clean), path.join(OUT, `${clean}.html`), path.join(OUT, clean, 'index.html')];
  for (const c of candidates) {
    if (existsSync(c) && statSync(c).isFile()) return c;
  }
  return null;
}

const files = await walk(OUT);
const html = new Map();
const ids = new Map();
for (const file of files) {
  const text = await readFile(file, 'utf8');
  html.set(file, text);
  ids.set(file, new Set([...text.matchAll(/\sid="([^"]+)"/g)].map((m) => decodeEntities(m[1]))));
}

const broken = [];
let checked = 0;
for (const [file, text] of html) {
  const pageUrl = urlOf(file);
  for (const m of text.matchAll(/<a\b[^>]*?\shref="([^"]*)"/g)) {
    const href = decodeEntities(m[1]);
    if (!href || /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) continue; // external, mailto:, …
    checked++;
    const url = new URL(href, `http://docs.local${pageUrl}`);
    let pathname;
    try {
      pathname = decodeURIComponent(url.pathname);
    } catch {
      broken.push({ page: pageUrl, href, reason: 'malformed URL' });
      continue;
    }
    const target = fileFor(pathname);
    if (!target) {
      broken.push({ page: pageUrl, href, reason: 'no such page or file' });
      continue;
    }
    const fragment = url.hash ? decodeURIComponent(url.hash.slice(1)) : '';
    if (fragment && target.endsWith('.html') && !ids.get(target)?.has(fragment)) {
      broken.push({ page: pageUrl, href, reason: `no element with id "${fragment}" on ${urlOf(target)}` });
    }
  }
}

if (broken.length > 0) {
  console.error(`✖ ${broken.length} broken internal link(s):`);
  for (const b of broken) console.error(`  ${b.page}  →  ${b.href}  (${b.reason})`);
  process.exit(1);
}
console.log(`✓ links: ${checked} internal links across ${files.length} pages are valid.`);

import { readFile } from 'node:fs/promises';
import path from 'node:path';
import type { Document } from 'fumadocs-openapi';
import { createOpenAPI } from 'fumadocs-openapi/server';
import { ferryServerUrl } from './shared';

/**
 * Schema id of Ferry's OpenAPI document. The generated API pages
 * (content/docs/reference/api/*.mdx, see scripts/generate.mjs) reference it.
 */
export const FERRY_OPENAPI_ID = 'ferry';

/**
 * openapi/ferry.json is the verbatim `ferryd --dump-openapi` output. It has no
 * `servers` entry (every ferryd has its own address), so the request examples
 * get one here: FERRY_DOCS_SERVER_URL, default http://127.0.0.1:7878.
 */
async function loadFerryDocument(): Promise<Document> {
  const file = path.join(process.cwd(), 'openapi', 'ferry.json');
  const doc = JSON.parse(await readFile(file, 'utf8')) as Document;
  return {
    ...doc,
    servers: [{ url: ferryServerUrl, description: 'Your Ferry server (ferryd --api-addr)' }],
  } as Document;
}

export const openapi = createOpenAPI({
  input: { [FERRY_OPENAPI_ID]: loadFerryDocument },
});

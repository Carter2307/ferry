'use client';
import type { MediaAdapter } from 'fumadocs-openapi';
import {
  createCodeUsageGeneratorRegistry,
  type CodeUsageGenerator,
  type CodeUsageGeneratorRegistry,
} from 'fumadocs-openapi/requests/generators';
import { registerDefault } from 'fumadocs-openapi/requests/generators/all';
import { createOpenAPIPage } from 'fumadocs-openapi/ui';

function text(body: unknown): string {
  return typeof body === 'string' ? body : JSON.stringify(body, null, 2);
}

/** Raw YAML bodies (`POST /api/v1/blueprints/apply` accepts a blueprint as YAML). */
const yaml: MediaAdapter = {
  encode: ({ body }) => text(body),
  generateExample({ body }, ctx) {
    const value = text(body);
    switch (ctx.lang) {
      case 'js':
        return `const body = ${JSON.stringify(value)};`;
      case 'python':
        return `body = ${JSON.stringify(value)}`;
      case 'go':
        if ('addImport' in ctx) (ctx.addImport as (name: string) => void)('strings');
        return `body := strings.NewReader(${JSON.stringify(value)})`;
      default:
        return undefined;
    }
  },
};

/** Binary uploads (`ferry up` sends a .tar.gz to /deploys/upload). */
const binary: MediaAdapter = {
  encode: ({ body }) => body as BodyInit,
  generateExample: () => undefined,
};

/**
 * Every `/api/v1` route needs the API token (`Authorization: Bearer <token>`), but
 * fumadocs-openapi leaves security schemes out of its request samples. The
 * generators below add the header and read the token from the `FERRY_TOKEN`
 * environment variable (the one the `ferry` CLI uses), so a copied sample works.
 * Webhooks (`/hooks/…`), `/healthz` and the OpenAPI document need no token.
 */
const TOKEN_PLACEHOLDER = '__FERRY_TOKEN__';
const QUOTED_PLACEHOLDER = `"Bearer ${TOKEN_PLACEHOLDER}"`;

/** Per sample language: the expression that builds the header value, and the import it needs. */
const tokenInLanguage: Record<string, { expression: string; addImport?: (code: string) => string }> = {
  js: { expression: '`Bearer ${process.env.FERRY_TOKEN}`' },
  python: { expression: '"Bearer " + os.environ["FERRY_TOKEN"]', addImport: (code) => `import os\n${code}` },
  go: {
    expression: '"Bearer "+os.Getenv("FERRY_TOKEN")',
    addImport: (code) => code.replace('  "net/http"\n', '  "net/http"\n  "os"\n'),
  },
  java: { expression: '"Bearer " + System.getenv("FERRY_TOKEN")' },
  csharp: { expression: '"Bearer " + Environment.GetEnvironmentVariable("FERRY_TOKEN")' },
  rust: { expression: 'format!("Bearer {}", std::env::var("FERRY_TOKEN").unwrap())' },
};

function withBearerToken(id: string, generator: CodeUsageGenerator): CodeUsageGenerator {
  return {
    ...generator,
    generate(data, context) {
      if (!data.url.includes('/api/v1/')) return generator.generate(data, context);
      const header = { Authorization: { value: `Bearer ${TOKEN_PLACEHOLDER}` }, ...data.header };
      const code = generator.generate({ ...data, header }, context);
      // cURL: the header is double-quoted, so the shell expands $FERRY_TOKEN.
      if (id === 'curl') return code.replace(TOKEN_PLACEHOLDER, '$FERRY_TOKEN');
      const language = tokenInLanguage[id];
      if (!language || !code.includes(QUOTED_PLACEHOLDER)) return code.replace(TOKEN_PLACEHOLDER, '<FERRY_TOKEN>');
      const replaced = code.replace(QUOTED_PLACEHOLDER, language.expression);
      return language.addImport ? language.addImport(replaced) : replaced;
    },
  };
}

function codeUsagesWithToken(): CodeUsageGeneratorRegistry {
  const registry = registerDefault(createCodeUsageGeneratorRegistry());
  for (const [id, generator] of Array.from(registry.map())) registry.add(id, withBearerToken(id, generator));
  return registry;
}

/**
 * Renders the generated API reference pages. The "try it" playground is
 * disabled: ferryd sends no CORS headers, so a browser can't call it from the
 * docs' origin — use the Swagger UI your server serves at /api/docs instead.
 */
export const OpenAPIPage = createOpenAPIPage({
  playground: { enabled: false },
  storageKeyPrefix: 'ferry-docs-openapi-',
  codeUsages: codeUsagesWithToken(),
  mediaAdapters: {
    'application/yaml': yaml,
    'text/yaml': yaml,
    'application/gzip': binary,
  },
});

import { llms, loader } from 'fumadocs-core/source';
import { lucideIconsPlugin } from 'fumadocs-core/source/plugins/lucide-icons';
import { metaSchema, pageSchema } from 'fumadocs-core/source/schema';
import { remarkMdxMermaid } from 'fumadocs-core/mdx-plugins';
import { applyMdxPreset } from 'fumadocs-mdx/config';
import { defineDocs } from 'fumadocs-mdx/macro';
import { docsRoute } from './shared';

// This module is also evaluated at build time by fumadocs-mdx (macro API):
// keep it free of heavy imports.
const docs = defineDocs({
  dir: 'content/docs',
  docs: {
    schema: pageSchema,
    // Default Fumadocs preset + ```mermaid code fences → <Mermaid chart="…" />.
    mdxOptions: applyMdxPreset({
      remarkPlugins: [remarkMdxMermaid],
    }),
    postprocess: {
      includeProcessedMarkdown: true,
    },
  },
  meta: {
    schema: metaSchema,
  },
});

// See https://fumadocs.dev/docs/headless/source-api for more info
export const source = loader({
  baseUrl: docsRoute,
  source: docs.toFumadocsSource(),
  // `icon: "Rocket"` in frontmatter / meta.json → lucide icon.
  plugins: [lucideIconsPlugin()],
});

export const docsLlms = llms(source, {
  renderPage: async (page) => `# ${page.data.title} (${page.url})

${await page.data.getText('processed')}`,
});

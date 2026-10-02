// Compiles a page and runs it with stand-in components that check the data of
// every figure: unknown icons, edges to nodes that do not exist, step ids that
// match nothing, labels too long for their box, unknown glossary terms.
// Used by scripts/check-ste.mjs; it returns the list of problems of one file.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { evaluate } from '@mdx-js/mdx';
import { createElement, Fragment } from 'react';
import * as runtime from 'react/jsx-runtime';
import { renderToStaticMarkup } from 'react-dom/server';
import remarkGfm from 'remark-gfm';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** The keys of the object literal that follows `marker` in a TypeScript file. */
function keysOf(file, marker) {
  const source = fs.readFileSync(path.join(root, file), 'utf8');
  const body = source.slice(source.indexOf(marker));
  const keys = new Set();
  let depth = 0;
  for (const line of body.split('\n')) {
    if (depth === 1) {
      const key = /^\s*(?:'([^']+)'|([A-Za-z][\w-]*))\s*:/.exec(line);
      if (key) keys.add(key[1] ?? key[2]);
    }
    // Braces inside a string (a definition with `${{…}}`) are text, not structure.
    const code = line.replace(/'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`/g, "''");
    depth += (code.match(/\{/g)?.length ?? 0) - (code.match(/\}/g)?.length ?? 0);
    if (depth <= 0 && keys.size) break;
  }
  return keys;
}

const ICONS = keysOf('components/mdx/icons.ts', 'export const figureIcons = {');
// Every file of lib/glossary/ adds terms; an id must exist only once.
const TERMS = new Set();
export const duplicateTerms = [];
for (const name of fs.readdirSync(path.join(root, 'lib/glossary')).sort()) {
  if (name === 'types.ts' || !name.endsWith('.ts')) continue;
  for (const key of keysOf(`lib/glossary/${name}`, 'export const terms = {')) {
    if (TERMS.has(key)) duplicateTerms.push(`${key} (lib/glossary/${name})`);
    TERMS.add(key);
  }
}
const TONES = new Set(['default', 'primary', 'success', 'warning', 'danger', 'muted']);

const MAX_LABEL_ICON = 15;
const MAX_LABEL = 20;
const MAX_NOTE = 18;
const MAX_COLUMNS = 4; // column index 3.x at most: the figure is 760px wide
const MAX_FLOW_STEPS = 7;
const MAX_FLOW_TITLE = 18;

export async function validate(file) {
  const problems = [];
  const say = (message) => problems.push(message);
  const pass = ({ children }) => createElement(Fragment, null, children);

  const icon = (name, where) => {
    if (name !== undefined && !ICONS.has(name)) say(`${where}: unknown icon "${name}" (see components/mdx/icons.ts)`);
  };
  const tone = (name, where) => {
    if (name !== undefined && !TONES.has(name)) say(`${where}: unknown tone "${name}"`);
  };

  const components = {
    Diagram({ nodes, edges = [], groups = [], steps }) {
      if (!Array.isArray(nodes) || !nodes.length) return say('Diagram: `nodes` is empty'), null;
      const ids = new Set();
      for (const node of nodes) {
        const where = `Diagram node "${node.id}"`;
        if (!node.id || ids.has(node.id)) say(`${where}: the id is missing or used twice`);
        ids.add(node.id);
        if (!Array.isArray(node.at) || node.at.length !== 2) say(`${where}: \`at\` must be [column, row]`);
        else if (node.at[0] > MAX_COLUMNS - 1 + 0.5) say(`${where}: column ${node.at[0]} is too far right (max ${MAX_COLUMNS - 0.5})`);
        const max = node.icon ? MAX_LABEL_ICON : MAX_LABEL;
        if (!node.label) say(`${where}: no label`);
        else if (node.label.length > max) say(`${where}: label "${node.label}" has ${node.label.length} characters (max ${max})`);
        if (node.note && node.note.length > MAX_NOTE) say(`${where}: note "${node.note}" has ${node.note.length} characters (max ${MAX_NOTE})`);
        icon(node.icon, where);
        tone(node.tone, where);
      }
      const edgeIds = new Set();
      for (const edge of edges) {
        const id = edge.id ?? `${edge.from}>${edge.to}`;
        if (edgeIds.has(id)) say(`Diagram edge "${id}": used twice (give one an \`id\`)`);
        edgeIds.add(id);
        for (const end of [edge.from, edge.to]) if (!ids.has(end)) say(`Diagram edge "${id}": no node "${end}"`);
        if (edge.label && edge.label.length > 16) say(`Diagram edge "${id}": label "${edge.label}" is too long (max 16)`);
        tone(edge.tone, `Diagram edge "${id}"`);
      }
      for (const group of groups) {
        if (!group.label) say('Diagram group: no label');
        if (group.around) {
          for (const id of group.around) if (!ids.has(id)) say(`Diagram group "${group.label}": no node "${id}"`);
        } else if (!Array.isArray(group.from) || !Array.isArray(group.to)) {
          say(`Diagram group "${group.label}": needs \`around\` (node ids), or \`from\` and \`to\``);
        }
      }
      if (steps) {
        if (steps.length < 2 || steps.length > 6) say(`Diagram: ${steps.length} steps (use 2 to 6)`);
        steps.forEach((step, i) => {
          const where = `Diagram step ${i + 1}`;
          if (!step.caption) say(`${where}: no caption`);
          for (const id of step.hide ?? []) if (!ids.has(id)) say(`${where}: hide: no node "${id}"`);
          for (const id of [...(step.flow ?? []), ...(step.cut ?? [])]) if (!edgeIds.has(id)) say(`${where}: no edge "${id}"`);
          for (const [id, t] of Object.entries(step.tones ?? {})) {
            if (!ids.has(id)) say(`${where}: tones: no node "${id}"`);
            tone(t, where);
          }
          for (const [id, note] of Object.entries(step.notes ?? {})) {
            if (!ids.has(id)) say(`${where}: notes: no node "${id}"`);
            if (note.length > MAX_NOTE) say(`${where}: note "${note}" has ${note.length} characters (max ${MAX_NOTE})`);
          }
        });
      }
      return null;
    },
    Flow({ steps }) {
      if (!Array.isArray(steps) || steps.length < 2) return say('Flow: needs 2 steps or more'), null;
      if (steps.length > MAX_FLOW_STEPS) say(`Flow: ${steps.length} steps (max ${MAX_FLOW_STEPS}: make two figures or two pages)`);
      const titles = new Set();
      steps.forEach((step, i) => {
        const where = `Flow step ${i + 1}`;
        if (!step.title) say(`${where}: no title`);
        else if (step.title.length > MAX_FLOW_TITLE) say(`${where}: title "${step.title}" has ${step.title.length} characters (max ${MAX_FLOW_TITLE})`);
        if (titles.has(step.title)) say(`${where}: title "${step.title}" used twice`);
        titles.add(step.title);
        if (!step.text) say(`${where}: no text`);
        icon(step.icon, where);
        tone(step.tone, where);
      });
      return null;
    },
    Terminal({ lines }) {
      if (!Array.isArray(lines) || !lines.length || lines.some((l) => typeof l !== 'string')) say('Terminal: `lines` must be a list of strings');
      else if (lines.length > 16) say(`Terminal: ${lines.length} lines (max 16)`);
      return null;
    },
    Term({ id, children }) {
      if (!TERMS.has(id)) say(`Term: "${id}" is not in lib/glossary.ts`);
      return createElement(Fragment, null, children ?? id);
    },
    Glossary: () => null,
    Mermaid: () => null,
    TypeTable: () => null,
    OpenAPIPage: () => null,
    Callout: pass,
    Cards: pass,
    Card: pass,
    Steps: pass,
    Step: pass,
    Tabs: pass,
    Tab: pass,
    Accordions: pass,
    Accordion: pass,
    Files: pass,
    Folder: pass,
    File: pass,
    Badge: pass,
  };

  // The frontmatter is YAML: a value with ": " or " #" must be in quotes, or
  // the page (and, in development, the whole site) fails to load.
  const front = /^---\n([\s\S]*?)\n---\n/.exec(fs.readFileSync(file, 'utf8'));
  if (!front) say('no frontmatter (title, description)');
  else {
    for (const key of ['title', 'description']) {
      const line = new RegExp(`^${key}:[ \\t]*(.*)$`, 'm').exec(front[1]);
      if (!line || !line[1].trim()) {
        say(`frontmatter: no ${key}`);
        continue;
      }
      const value = line[1].trim();
      const quoted = /^(".*"|'.*')$/.test(value);
      if (!quoted && (/: |:$| #/.test(value) || /^[[{>|*&!%@`]/.test(value))) {
        say(`frontmatter: put the ${key} in double quotes (it has ": ", " #" or a special first character)`);
      }
    }
  }

  try {
    const source = fs.readFileSync(file, 'utf8').replace(/^---\n[\s\S]*?\n---\n/, '');
    const { default: Content } = await evaluate(source, {
      ...runtime,
      baseUrl: pathToFileURL(file).href,
      remarkPlugins: [remarkGfm],
    });
    renderToStaticMarkup(createElement(Content, { components }));
  } catch (error) {
    const at = error.line ? ` (line ${error.line}${error.column ? `:${error.column}` : ''}, counted after the frontmatter)` : '';
    say(`does not compile${at}: ${String(error.message ?? error).split('\n')[0]}`);
  }
  return problems;
}

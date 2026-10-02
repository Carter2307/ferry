#!/usr/bin/env node
// Measures how simple a page is to read, with the main writing rules of
// ASD-STE100 (Simplified Technical English) and the page budget of
// AUTHORING.md.
//
//   node scripts/check-ste.mjs                      every hand-written page
//   node scripts/check-ste.mjs content/docs/concepts/deploys
//   node scripts/check-ste.mjs -v <path>            also print each sentence that fails
//   node scripts/check-ste.mjs --strict <path>      exit 1 when a page is below the targets
//
// A page passes when at least 80% of its sentences obey every rule below, when
// it has a picture or a script, when it stays inside the word budget, and when
// its figures are correct (scripts/check-figures.mjs).
//
// This is a heuristic, not the STE standard: it knows the sentence rules and a
// short list of words that STE does not approve. It does not have the STE
// dictionary. Read its output as a guide.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { duplicateTerms, validate } from './check-figures.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const verbose = args.includes('-v');
const strict = args.includes('--strict');
const targets = args.filter((a) => !a.startsWith('-'));

export const TARGET = 0.8; // share of sentences that must pass
export const MAX_WORDS = 220; // words a reader sees in sentences (accordions excluded)
export const MAX_TABLE_WORDS = 260; // words in tables
export const MAX_HIDDEN_WORDS = 260; // words in closed accordions (details)
const MAX_DESCRIPTIVE = 25; // STE rule 6.1
const MAX_INSTRUCTION = 20; // STE rule 5.1
const MAX_PARAGRAPH = 6; // STE rule 6.3 (sentences)

// Generated from the Rust code: not written by hand, not measured.
const GENERATED = [/^reference\/api\//, /^reference\/cli\.mdx$/, /^reference\/server-options\.mdx$/];

// Verbs that start an instruction ("Run the command.").
const IMPERATIVES = new Set(
  `add apply ask build call cancel change check choose click close connect copy create delete deploy do download edit
   enter find follow give go install keep link look make open paste pass point press pull push put quote read remove
   replace restart roll run save scale select send set show sign start stop tell try type update upload use wait write`
    .split(/\s+/)
    .filter(Boolean),
);

// Words and phrases that STE does not approve, with what to write.
const UNAPPROVED = [
  [/\bmay\b/i, 'may → can'],
  [/\bmight\b/i, 'might → can / it is possible that'],
  [/\bshould\b/i, 'should → must (or give the instruction)'],
  [/\bvia\b/i, 'via → through / with'],
  [/\bonce\b/i, 'once → when / after (at once → immediately)'],
  [/\bsince\b/i, 'since → because / after'],
  [/\bin order to\b/i, 'in order to → to'],
  [/\bas well as\b/i, 'as well as → and'],
  [/\be\.g\.|\bi\.e\.|\betc\b/i, 'e.g. / i.e. / etc. → for example / that is / name the items'],
  [/\butili[sz]e[sd]?\b/i, 'utilize → use'],
  [/\bensures?\b/i, 'ensure → make sure'],
  [/\b(simply|just|basically|actually|really|very|quite|easily|obviously|of course)\b/i, 'filler word → remove it'],
  [/\bhowever\b/i, 'however → but'],
  [/\btherefore\b/i, 'therefore → thus / as a result'],
  [/\bwhether\b/i, 'whether → if'],
  [/\bperform(s|ed)?\b/i, 'perform → do'],
  [/\bprior to\b/i, 'prior to → before'],
  [/\bupon\b/i, 'upon → when / on'],
  [/\b(obtain|retrieve)s?\b/i, 'obtain / retrieve → get'],
  [/\bterminates?\b/i, 'terminate → stop'],
  [/\badditional\b/i, 'additional → more'],
  [/\bleverag(e|es|ed)\b/i, 'leverage → use'],
  [/\b(spin|spins|spun) up\b|\bkick(s|ed)? off\b|\bunder the hood\b|\bout of the box\b|\bon the fly\b/i, 'idiom → plain words'],
  [/\b(handy|leftovers?|tweak|grab|nuke|gotcha)\b/i, 'informal word → plain word'],
  [/\b(don't|doesn't|didn't|can't|won't|isn't|aren't|wasn't|weren't|hasn't|haven't|it's|that's|there's|you're|you'll|you've|we're|we'll|they're)\b/i, 'contraction → write the full words'],
];

// Words in -ing that are names of things, not verb forms.
const ING_NAMES = new Set(
  `thing things nothing anything something everything during string strings setting settings warning warnings
   networking scaling routing balancing healing logging ring king bring spring sibling siblings ping pending
   building staging billing meaning listing morning evening ceiling
   heading headings coding polling troubleshooting forwarding`
    .split(/\s+/)
    .filter(Boolean),
);
const PARTICIPLES =
  'built|sent|kept|made|run|shown|written|taken|given|known|done|set|put|chosen|found|held|lost|read|told|seen|left|bound|hidden|paid|sold|stuck|cut|shut|drawn|thrown|broken|frozen|stored|begun|gone|won';
const NOT_PARTICIPLES = /^(need|speed|feed|red|bed|shed|embed|indeed|proceed|exceed|succeed|hundred|shared|named|limited)$/i;

function listFiles(target) {
  const full = path.resolve(root, target);
  if (!fs.existsSync(full)) return [];
  if (fs.statSync(full).isFile()) return full.endsWith('.mdx') ? [full] : [];
  return fs
    .readdirSync(full, { withFileTypes: true })
    .flatMap((entry) => listFiles(path.join(target, entry.name)));
}

const words = (text) => text.split(/\s+/).filter((w) => /[A-Za-z0-9]/.test(w));

/** MDX → the units a reader reads: paragraphs (lists of sentences), table cells, and the page facts. */
export function extract(source) {
  let text = source;
  const front = /^---\n([\s\S]*?)\n---\n/.exec(text);
  const description = front ? /^description:\s*["']?(.*?)["']?\s*$/m.exec(front[1])?.[1] : undefined;
  if (front) text = text.slice(front[0].length);

  const pictures =
    (text.match(/<(Diagram|Flow|Terminal|Mermaid)\b/g)?.length ?? 0) +
    (text.match(/^```/gm)?.length ?? 0) / 2 +
    (text.match(/!\[[^\]]*\]\(/g)?.length ?? 0);

  // Sentences inside figures (captions, step texts): they count as reading.
  const figureTexts = [];
  for (const match of text.matchAll(/\b(caption|text)\s*[:=]\s*(['"`])((?:\\.|(?!\2).)*)\2/g)) {
    figureTexts.push(match[3].replace(/\\(['"`])/g, '$1'));
  }

  text = text
    .replace(/^```[\s\S]*?^```\s*$/gm, '\n') // code blocks
    .replace(/^(import|export)\s.*$/gm, '')
    .replace(/<Accordions>[\s\S]*?<\/Accordions>/g, (block) => `\n@@ACCORDION@@${block}@@END@@\n`);

  const hidden = [];
  text = text.replace(/@@ACCORDION@@([\s\S]*?)@@END@@/g, (_, block) => {
    hidden.push(block);
    return '\n';
  });

  const clean = (block) =>
    block
      .replace(/<[A-Z][A-Za-z]*\b[^<>]*?\{[\s\S]*?\}\s*\/>/g, '\n') // <Diagram … /> with data props
      .replace(/<[A-Z][A-Za-z.]*\b[^<>]*\/>/g, ' TERM ') // <Term id="x" />
      .replace(/<\/?[A-Za-z][^<>]*>/g, '') // other tags: keep what is between them
      .replace(/\{\/\*[\s\S]*?\*\/\}/g, '')
      .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
      .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/`[^`\n]*`/g, 'CODE')
      .replace(/\*\*|__|(?<![A-Za-z0-9])[*_]|[*_](?![A-Za-z0-9])/g, '');

  const visible = clean(text);
  const paragraphs = [];
  const cells = [];
  let headings = 0;
  for (const block of visible.split(/\n\s*\n/)) {
    const lines = block.split('\n').map((l) => l.trim()).filter(Boolean);
    if (!lines.length) continue;
    if (lines.every((l) => l.startsWith('|'))) {
      for (const line of lines) {
        if (/^\|[\s:|-]+\|$/.test(line)) continue;
        for (const cell of line.split('|').map((c) => c.trim()).filter(Boolean)) cells.push(cell);
      }
      continue;
    }
    const prose = [];
    for (const line of lines) {
      if (/^#{1,6}\s/.test(line)) {
        headings += 1;
        continue;
      }
      const item = /^(?:[-*+]|\d+\.)\s+(.*)$/.exec(line);
      if (item) {
        // every list item is a unit of its own
        paragraphs.push(sentences(item[1]));
        continue;
      }
      prose.push(line);
    }
    if (prose.length) paragraphs.push(sentences(prose.join(' ')));
  }
  if (description) paragraphs.push(sentences(description));
  for (const t of figureTexts) paragraphs.push(sentences(t));

  // Accordions are closed when the page opens: their sentences obey the rules,
  // but they are not part of what the reader sees first.
  const hiddenParagraphs = [];
  for (const block of hidden) {
    for (const part of clean(block).split(/\n\s*\n|\n(?=\s*(?:[-*+]|\d+\.)\s)/)) {
      const line = part.replace(/^\s*(?:[-*+]|\d+\.)\s+/, '').trim();
      if (line && !line.startsWith('|')) hiddenParagraphs.push(sentences(line));
    }
  }
  return {
    paragraphs: paragraphs.filter((p) => p.length),
    hiddenParagraphs: hiddenParagraphs.filter((p) => p.length),
    cells,
    pictures,
    headings,
  };
}

function sentences(text) {
  return text
    .replace(/\s+/g, ' ')
    .split(/(?<=[.?!])\s+(?=[A-Z"“`(]|CODE|TERM)/)
    .map((s) => s.trim())
    .filter((s) => words(s).length > 0);
}

/** The rules a sentence breaks (an empty list: the sentence passes). */
export function check(sentence) {
  const problems = [];
  const w = words(sentence);
  const first = w[0]?.toLowerCase().replace(/[^a-z]/g, '') ?? '';
  const instruction = IMPERATIVES.has(first);
  const max = instruction ? MAX_INSTRUCTION : MAX_DESCRIPTIVE;
  if (w.length > max) problems.push(`${w.length} words (max ${max}${instruction ? ' for an instruction' : ''})`);
  if (/;/.test(sentence)) problems.push('two ideas in one sentence (;)');

  const passive = new RegExp(
    String.raw`\b(?:is|are|was|were|be|been|being)\s+(?:not\s+)?(?:\w+ly\s+)?(\w+ed|${PARTICIPLES})\b`,
    'i',
  ).exec(sentence);
  if (passive && !NOT_PARTICIPLES.test(passive[1])) problems.push(`passive voice ("${passive[0]}")`);

  for (const [pattern, advice] of UNAPPROVED) if (pattern.test(sentence)) problems.push(advice);

  const ing = w
    .map((x) => x.toLowerCase().replace(/[^a-z-]/g, ''))
    .filter((x) => /[a-z]{2,}ing$/.test(x) && !ING_NAMES.has(x.split('-').pop()));
  if (ing.length) problems.push(`-ing form (${[...new Set(ing)].join(', ')})`);
  return problems;
}

export function measure(source) {
  const { paragraphs, hiddenParagraphs, cells, pictures } = extract(source);
  const failures = [];
  let total = 0;
  let passed = 0;
  let readingWords = 0;
  let hiddenWords = 0;
  for (const paragraph of hiddenParagraphs) {
    for (const sentence of paragraph) {
      total += 1;
      hiddenWords += words(sentence).length;
      const problems = check(sentence);
      if (problems.length) failures.push({ sentence, problems });
      else passed += 1;
    }
  }
  for (const paragraph of paragraphs) {
    if (paragraph.length > MAX_PARAGRAPH) {
      failures.push({ sentence: paragraph[0], problems: [`paragraph of ${paragraph.length} sentences (max ${MAX_PARAGRAPH})`] });
    }
    for (const sentence of paragraph) {
      total += 1;
      readingWords += words(sentence).length;
      const problems = check(sentence);
      if (problems.length) failures.push({ sentence, problems });
      else passed += 1;
    }
  }
  const tableWords = cells.reduce((n, cell) => n + words(cell).length, 0);
  for (const cell of cells) {
    // A cell that is a full sentence obeys the same rules.
    if (words(cell).length < 6) continue;
    for (const sentence of sentences(cell)) {
      total += 1;
      const problems = check(sentence);
      if (problems.length) failures.push({ sentence, problems });
      else passed += 1;
    }
  }
  // The glossary is a list of definitions: it needs no picture.
  const exempt = /<Glossary\b/.test(source);
  return { score: total ? passed / total : 1, total, readingWords, tableWords, hiddenWords, pictures, exempt, failures };
}

async function main() {
  const files = (targets.length ? targets : ['content/docs'])
    .flatMap(listFiles)
    .filter((file) => !GENERATED.some((g) => g.test(path.relative(path.join(root, 'content/docs'), file))))
    .sort();
  if (!files.length) {
    console.error('No .mdx file found.');
    process.exit(2);
  }
  let below = 0;
  let allPassed = 0;
  let allTotal = 0;
  for (const file of files) {
    const result = measure(fs.readFileSync(file, 'utf8'));
    const notes = [];
    if (result.score < TARGET) notes.push(`STE ${Math.round(result.score * 100)}% < ${TARGET * 100}%`);
    if (result.readingWords > MAX_WORDS) notes.push(`${result.readingWords} words > ${MAX_WORDS}`);
    if (result.tableWords > MAX_TABLE_WORDS) notes.push(`${result.tableWords} table words > ${MAX_TABLE_WORDS}`);
    if (result.hiddenWords > MAX_HIDDEN_WORDS) notes.push(`${result.hiddenWords} words in accordions > ${MAX_HIDDEN_WORDS}`);
    if (!result.pictures && !result.exempt) notes.push('no picture or script');
    const figures = await validate(file);
    if (figures.length) notes.push(`${figures.length} figure problem${figures.length > 1 ? 's' : ''}`);
    if (notes.length) below += 1;
    allPassed += Math.round(result.score * result.total);
    allTotal += result.total;
    const name = path.relative(root, file);
    console.log(
      `${notes.length ? '✗' : '✓'} ${String(Math.round(result.score * 100)).padStart(3)}%  ${String(result.readingWords).padStart(4)} words  ${name}${notes.length ? `  — ${notes.join(', ')}` : ''}`,
    );
    for (const problem of figures) console.log(`      ! ${problem}`);
    if (verbose) {
      for (const failure of result.failures) {
        console.log(`      · ${failure.sentence.slice(0, 110)}${failure.sentence.length > 110 ? '…' : ''}`);
        console.log(`        ${failure.problems.join(' | ')}`);
      }
    }
  }
  console.log(
    `\n${files.length} pages, ${files.length - below} inside the targets. All sentences together: ${allTotal ? Math.round((allPassed / allTotal) * 100) : 100}% pass.`,
  );
  if (duplicateTerms.length) console.log(`Glossary: these ids exist twice: ${duplicateTerms.join(', ')}`);
  if (strict && (below || duplicateTerms.length)) process.exit(1);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();

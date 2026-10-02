import defaultMdxComponents from 'fumadocs-ui/mdx';
import { Accordion, Accordions } from 'fumadocs-ui/components/accordion';
import { File, Files, Folder } from 'fumadocs-ui/components/files';
import { Step, Steps } from 'fumadocs-ui/components/steps';
import { Tab, Tabs } from 'fumadocs-ui/components/tabs';
import { TypeTable } from 'fumadocs-ui/components/type-table';
import type { MDXComponents } from 'mdx/types';
import { Badge } from './mdx/badge';
import { Callout } from './mdx/callout';
import { Card, Cards } from './mdx/card';
import { Diagram } from './mdx/diagram';
import { Flow } from './mdx/flow';
import { Mermaid } from './mdx/mermaid';
import { Glossary, Term } from './mdx/term';
import { Terminal } from './mdx/terminal';

/**
 * Components available in every MDX page (no import needed):
 * Callout, Cards/Card, Steps/Step, Tabs/Tab, Accordions/Accordion,
 * Files/Folder/File, TypeTable, Badge, Mermaid (+ ```mermaid fences),
 * and the figures: Flow, Diagram, Terminal, Term, Glossary (see AUTHORING.md).
 */
export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    Callout,
    Card,
    Cards,
    Steps,
    Step,
    Tabs,
    Tab,
    Accordions,
    Accordion,
    Files,
    Folder,
    File,
    TypeTable,
    Badge,
    Mermaid,
    Flow,
    Diagram,
    Terminal,
    Term,
    Glossary,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}

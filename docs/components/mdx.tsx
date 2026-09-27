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
import { Mermaid } from './mdx/mermaid';

/**
 * Components available in every MDX page (no import needed):
 * Callout, Cards/Card, Steps/Step, Tabs/Tab, Accordions/Accordion,
 * Files/Folder/File, TypeTable, Badge, Mermaid (+ ```mermaid fences).
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
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}

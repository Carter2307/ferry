/**
 * The glossary: one short definition for each technical name in the docs.
 * `<Term id="container" />` shows the definition in a tooltip, and
 * /docs/glossary lists every term.
 *
 * The terms live in lib/glossary/: `core.ts` for the terms that every section
 * uses, then one file per section. See AUTHORING.md.
 */
import { terms as core } from './glossary/core';
import { terms as servicesBuilds } from './glossary/services-builds';
import { terms as networkEnv } from './glossary/network-env';
import { terms as dataJobs } from './glossary/data-jobs';
import { terms as limitsLogs } from './glossary/limits-logs';
import { terms as architecture } from './glossary/architecture';
import { terms as security } from './glossary/security';
import { terms as start } from './glossary/start';
import { terms as startTools } from './glossary/start-tools';
import { terms as guidesGit } from './glossary/guides-git';
import { terms as guidesProd } from './glossary/guides-prod';
import { terms as guidesDomains } from './glossary/guides-domains';
import { terms as reference } from './glossary/reference';
import { terms as contributing } from './glossary/contributing';
import type { GlossaryEntry } from './glossary/types';

export type { GlossaryEntry };

export const glossary = {
  ...core,
  ...servicesBuilds,
  ...networkEnv,
  ...dataJobs,
  ...limitsLogs,
  ...architecture,
  ...security,
  ...start,
  ...startTools,
  ...guidesGit,
  ...guidesProd,
  ...guidesDomains,
  ...reference,
  ...contributing,
} satisfies Record<string, GlossaryEntry>;

export type TermId = keyof typeof glossary;

export function glossaryEntry(id: string): GlossaryEntry | undefined {
  return (glossary as Record<string, GlossaryEntry>)[id];
}

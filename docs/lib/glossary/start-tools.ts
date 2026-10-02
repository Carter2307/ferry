import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- The dashboard -------------------------------------------------------
  'icon-rail': {
    term: 'icon rail',
    definition: 'The column of icons on the left side of the dashboard. Each icon opens one main page.',
    see: ['dashboard'],
  },
  breadcrumb: {
    term: 'breadcrumb',
    definition: 'The line at the top of a page that shows where you are, for example the server, then the service.',
  },
  'command-palette': {
    term: 'command palette',
    definition: 'A search box that opens on top of the page. You type a name, then it opens a page or starts an action.',
  },
  'browser-session': {
    term: 'session',
    definition: 'The time that one browser stays signed in to the dashboard. A cookie in the browser identifies it.',
  },
  'dry-run': {
    term: 'dry run',
    definition: 'A test of a blueprint. It shows what Ferry will create or change, and it changes nothing.',
    see: ['blueprint'],
  },
  openapi: {
    term: 'OpenAPI document',
    definition: 'A file that describes each address of an API in a standard format. Tools such as Swagger UI read it.',
    see: ['api'],
  },

  // --- The CLI -------------------------------------------------------------
  terminal: {
    term: 'terminal',
    definition: 'The window where you type commands and read their output.',
    see: ['cli'],
  },
  flag: {
    term: 'flag',
    definition: 'An option that you add to a command. It starts with `--`, for example `--json`.',
  },
  'shell-variable': {
    term: 'variable of the shell',
    definition: 'A value with a name that your shell gives to each command, for example `FERRY_TOKEN`. You set it with `export`.',
    see: ['env-var'],
  },
  'config-file': {
    term: 'config file',
    definition: 'The file where `ferry login` saves the server URL and the API token: `~/.config/ferry/config.json`.',
    see: ['api-token'],
  },
  json: {
    term: 'JSON',
    definition: 'A text format for data that programs read. Example: `{"name": "api", "state": "live"}`.',
  },
  stdout: {
    term: 'stdout',
    definition: 'Standard output: the channel where a program writes its results. A pipe (`|`) sends it to the next command.',
    see: ['stderr'],
  },
  stderr: {
    term: 'stderr',
    definition: 'Standard error: the channel where a program writes its errors and its progress messages.',
    see: ['stdout'],
  },
  'exit-code': {
    term: 'exit code',
    definition: 'The number that a command gives to the shell when it stops. `0` is a success. Each other number is a failure.',
  },
} satisfies Record<string, GlossaryEntry>;

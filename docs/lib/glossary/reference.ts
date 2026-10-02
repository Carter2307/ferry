import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- The blueprint file --------------------------------------------------
  yaml: {
    term: 'YAML',
    definition: 'A text format for settings. Each line is `key: value`. The indentation shows what is inside what.',
    see: ['blueprint'],
  },
  'yaml-key': {
    term: 'key',
    definition: 'The name on the left of the `:` in a YAML file. Its value is on the right, for example `name: api`.',
    see: ['yaml'],
  },
  'top-level-key': {
    term: 'top-level key',
    definition: 'A key with no indentation. A blueprint has four: `envVarGroups`, `databases`, `services` and `projects`.',
    see: ['yaml-key'],
  },
  mapping: {
    term: 'mapping',
    definition: 'A group of `key: value` pairs in YAML. Each entry of `services` is a mapping.',
    see: ['yaml'],
  },
  scalar: {
    term: 'scalar',
    definition: 'One simple value in YAML: a text, a number or a boolean. A list and a mapping are not scalars.',
    see: ['yaml'],
  },
  boolean: {
    term: 'boolean',
    definition: 'A value that is true or false.',
  },
  'yaml-anchor': {
    term: 'YAML anchor',
    definition: 'A name for a block of YAML, written `&defaults`. `<<: *defaults` copies the block to another place of the file.',
    see: ['yaml'],
  },
  'blueprint-entry': {
    term: 'entry',
    definition: 'One item of a list in a blueprint. An entry describes one service, one database, one env group or one variable.',
    see: ['blueprint'],
  },
  resource: {
    term: 'resource',
    definition: 'A thing that Ferry controls and that a blueprint can describe: a service, a datastore or an env group.',
  },
  apply: {
    term: 'apply',
    definition: 'The operation that reads a blueprint, then creates what is not there and updates what changed.',
    see: ['blueprint', 'dry-run'],
  },
  'blueprint-action': {
    term: 'action',
    definition: 'What an apply does to one resource: `create`, `update` or `unchanged`.',
    see: ['apply'],
  },
  idempotent: {
    term: 'idempotent',
    definition: 'An operation is idempotent when a second run with the same input changes nothing.',
  },
  validation: {
    term: 'validation',
    definition: 'The test of each value against the rules, before Ferry writes something.',
  },
  'ferry-extension': {
    term: 'Ferry key',
    definition: 'A blueprint key that Ferry adds to the `render.yaml` format: `port`, `memoryLimit` and `cpuLimit`. Render has no such key.',
    see: ['blueprint'],
  },
  'generated-secret': {
    term: 'generated secret',
    definition: 'A random value of 64 hex characters (256 bits). Ferry makes it for a variable that has `generateValue: true`.',
  },
  'camel-case': {
    term: 'camelCase',
    definition: 'A way to write a name of many words with no space. Each word after the first starts with a capital letter: `healthCheckPath`.',
    see: ['snake-case'],
  },
  // --- References ----------------------------------------------------------
  'reference-kind': {
    term: 'kind',
    definition: 'The first part of a reference. It tells if the name is the name of a datastore or of a service.',
    see: ['reference'],
  },
  'reference-property': {
    term: 'property',
    definition: 'The last part of a reference. It tells which value you want, for example `host` or `connectionString`.',
    see: ['reference'],
  },
  escape: {
    term: 'escape',
    definition: 'To write a special text so that a program reads it as plain text. `$${{ name }}` gives the literal text `${{ name }}`.',
  },
  // --- The API -------------------------------------------------------------
  'rest-api': {
    term: 'REST API',
    definition: 'An API that uses plain HTTP requests. The URL names a resource. The method (`GET`, `POST`, `DELETE`) names the action.',
    see: ['api', 'http-method'],
  },
  'base-url': {
    term: 'base URL',
    definition: 'The start of each URL of the API: the address of the server, then `/api/v1`.',
  },
  curl: {
    term: 'curl',
    definition: 'A command that sends one HTTP request and prints the answer.',
  },
  'bearer-token': {
    term: 'bearer token',
    definition: 'A token that a client sends in the header `Authorization: Bearer <token>`.',
    see: ['api-token'],
  },
  'http-method': {
    term: 'method',
    definition: 'The verb of an HTTP request. `GET` reads. `POST` creates or starts. `PUT` and `PATCH` change. `DELETE` removes.',
  },
  'request-body': {
    term: 'body',
    definition: 'The data of an HTTP request or answer. It comes after the headers.',
    see: ['header'],
  },
  'content-type': {
    term: 'content type',
    definition: 'The value of the header `Content-Type`. It tells the format of the body, for example `application/json`.',
    see: ['request-body'],
  },
  'snake-case': {
    term: 'snake_case',
    definition: 'A way to write a name of many words: lower case, with `_` between the words, for example `base_domain`.',
    see: ['camel-case'],
  },
  'rfc-3339': {
    term: 'RFC 3339',
    definition: 'A standard text format for a date and a time, for example `2026-09-27T07:38:34.231568Z`.',
    see: ['timestamp'],
  },
  pagination: {
    term: 'pagination',
    definition: 'An API with pagination gives a long list in parts, one page for each request. The Ferry API gives the full list.',
  },
  'error-code': {
    term: 'error code',
    definition: 'The `code` field of an API error, for example `not_found`. It is stable: programs can test it.',
    see: ['status-code'],
  },
  'event-stream': {
    term: 'stream',
    definition: 'An HTTP answer that stays open. The server adds data to it while the client reads.',
    see: ['sse'],
  },
  'event-source': {
    term: 'EventSource',
    definition: 'The object of a browser that reads a Server-Sent Events stream. It connects again by itself. It cannot set headers.',
    see: ['sse'],
  },
  'keep-alive': {
    term: 'keep-alive comment',
    definition: 'The line `: keep-alive`. The server sends it on a stream that has nothing new, to keep the connection open.',
    see: ['event-stream'],
  },
  'resource-id': {
    term: 'id',
    definition: 'The identifier that Ferry gives to a resource: a prefix and 20 hex characters, for example `srv-60f5973b037e4478ae7b`.',
  },
  delivery: {
    term: 'delivery',
    definition: 'One webhook request from GitHub. The header `X-GitHub-Delivery` gives its id.',
    see: ['webhook'],
  },
} satisfies Record<string, GlossaryEntry>;

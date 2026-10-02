import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- The parts of ferryd -------------------------------------------------
  crate: {
    term: 'crate',
    definition: 'A package of Rust code. A crate is a library that other crates use, or a program.',
  },
  engine: {
    term: 'engine',
    definition: 'The part of `ferryd` that runs the deploys, the reconciler, the cron jobs and the datastores. Its crate is `ferry-engine`.',
    see: ['reconciler', 'deploy-worker'],
  },
  store: {
    term: 'store',
    definition: 'The part of Ferry that reads and writes the SQLite file `ferry.db`. It has the desired state.',
    see: ['sqlite', 'desired-state'],
  },
  trait: {
    term: 'trait',
    definition: 'In Rust, a list of functions that a type must have. Code that calls a trait does not know which type answers.',
  },
  'route-table': {
    term: 'route table',
    definition: 'A list in the memory of `ferryd`. For each hostname, it gives the addresses of the instances. The proxy reads it for each request.',
    see: ['proxy', 'upstream'],
  },
  upstream: {
    term: 'upstream',
    definition: 'An address to which the proxy sends requests: one instance, as `127.0.0.1:<host port>`.',
    see: ['route-table'],
  },
  'docker-engine-api': {
    term: 'Docker Engine API',
    definition: 'The HTTP interface of Docker. A program calls it to create, start, stop and read containers, images, volumes and networks.',
    see: ['docker'],
  },
  'container-label': {
    term: 'label',
    definition: 'A key and a value that Docker keeps on a container. Ferry reads the labels to find its own containers.',
  },

  // --- The deploy pipeline -------------------------------------------------
  'deploy-queue': {
    term: 'deploy queue',
    definition: 'The deploys of one service that wait for their turn. Their status is `queued`.',
    see: ['deploy-worker'],
  },
  'deploy-worker': {
    term: 'deploy worker',
    definition: 'A background task of the engine. It runs the deploys of one service, one at a time, in order.',
    see: ['deploy-queue', 'build-slot'],
  },
  'build-slot': {
    term: 'build slot',
    definition: 'The right to build one image. The server has 2 build slots by default (`--build-concurrency`), thus it builds 2 images at a time.',
  },
  'service-lock': {
    term: 'service lock',
    definition: 'A lock that each service has. Only the operation that holds it can change the containers of the service.',
    see: ['reconciler'],
  },
  'blue-green': {
    term: 'blue-green deploy',
    definition: 'A deploy that starts the new instances next to the old instances, then moves the traffic. It is the default.',
    see: ['zero-downtime', 'recreate'],
  },
  recreate: {
    term: 'recreate',
    definition: 'A deploy that stops the old instance before it starts the new instance. Ferry uses it for a service with a disk.',
    see: ['disk'],
  },

  'published-port': {
    term: 'published port',
    definition: 'A port of the host that Docker connects to a port of a container. Ferry publishes ports only on `127.0.0.1`.',
    see: ['host-port', 'localhost'],
  },

  // --- The reconciler ------------------------------------------------------
  'oom-watcher': {
    term: 'OOM watcher',
    definition: 'A task of the engine that listens to the `oom` events of Docker. It writes a warning in the server log for each kill.',
    see: ['oom'],
  },
  sni: {
    term: 'SNI',
    definition: 'Server Name Indication: at the start of an HTTPS connection, the client gives the hostname. The server then chooses the certificate of that hostname.',
    see: ['certificate'],
  },
  backoff: {
    term: 'backoff',
    definition: 'A wait before a new try. The wait becomes longer after each failure.',
  },

} satisfies Record<string, GlossaryEntry>;

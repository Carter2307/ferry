import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Datastores ----------------------------------------------------------
  postgres: {
    term: 'Postgres',
    definition: 'A database that keeps data in tables. Apps read and change the data with SQL.',
    see: ['datastore'],
  },
  redis: {
    term: 'Redis',
    definition: 'A database that keeps keys and values in memory. Apps use it as a cache or as a queue.',
    see: ['datastore'],
  },
  alpine: {
    term: 'Alpine',
    definition: 'A small Linux system. An image that is built on Alpine uses less disk space.',
    see: ['image'],
  },
  'internal-url': {
    term: 'internal URL',
    definition: 'The connection string for the private network. Its host is the name of the datastore, for example `app-db:5432`.',
    see: ['connection-string', 'private-network'],
  },
  'external-url': {
    term: 'external URL',
    definition: 'The connection string for the server itself. Its host is `127.0.0.1`, with a port of the server.',
    see: ['connection-string', 'host-port'],
  },
  'host-port': {
    term: 'host port',
    definition: 'A port of the server that Docker connects to a port of a container.',
    see: ['port'],
  },
  psql: {
    term: 'psql',
    definition: 'The command-line client of Postgres. It sends SQL to a database.',
  },
  backup: {
    term: 'backup',
    definition: 'A copy of your data that you keep in a different place. You use it to get the data back after a loss.',
  },
  replica: {
    term: 'replica',
    definition: 'A second database server that keeps a copy of the data of the first server.',
  },
  'high-availability': {
    term: 'high availability',
    definition: 'A setup that stays online when one container or one machine fails.',
    see: ['replica'],
  },
  'connection-pooler': {
    term: 'connection pooler',
    definition: 'A program between your apps and a database. With it, many app connections share a small number of database connections.',
  },

  // --- Jobs ----------------------------------------------------------------
  'job-run': {
    term: 'run',
    definition: 'One execution of a job: one new container, one command, one log and one exit code.',
    see: ['job', 'one-off-job', 'cron-job'],
  },
  'one-off-job': {
    term: 'one-off job',
    definition: 'A command that you start one time with `ferry run`. It runs in a new container of a service, then stops.',
    see: ['job-run'],
  },
  'cron-expression': {
    term: 'cron expression',
    definition: 'Five fields that give a schedule: minute, hour, day of month, month and day of week.',
    see: ['cron-schedule'],
  },
  scheduler: {
    term: 'scheduler',
    definition: 'The part of Ferry that starts a run of a cron job at each time of its schedule.',
    see: ['cron-schedule'],
  },
  sigterm: {
    term: 'SIGTERM',
    definition: 'The signal that asks a program to stop. The program can save its work first.',
    see: ['sigkill', 'grace-period'],
  },
  'grace-period': {
    term: 'grace period',
    definition: 'The time that a container has to stop by itself. After that time, Docker stops it by force.',
    see: ['sigterm', 'sigkill'],
  },

  // --- Disks, scaling and suspend -----------------------------------------
  'live-deploy': {
    term: 'live deploy',
    definition: 'The deploy of a service that gets the traffic now. A service has one live deploy at most.',
    see: ['deploy', 'snapshot'],
  },
  scaling: {
    term: 'scaling',
    definition: 'A change of the number of instances of a service.',
    see: ['instance', 'autoscaling'],
  },
  autoscaling: {
    term: 'autoscaling',
    definition: 'A system that changes the number of instances by itself when the load changes. Ferry does not have it.',
    see: ['scaling'],
  },
  degraded: {
    term: 'degraded',
    definition: 'The state of a service that runs fewer instances than you asked for.',
    see: ['reconciler'],
  },
  'mount-path': {
    term: 'mount path',
    definition: 'The folder in the container where the disk shows, for example `/data`.',
    see: ['disk'],
  },
  suspend: {
    term: 'suspend',
    definition: 'To stop all the instances of a service and keep its settings, its deploys and its data.',
    see: ['resume'],
  },
  resume: {
    term: 'resume',
    definition: 'To start the instances of a suspended service again, from the live deploy.',
    see: ['suspend'],
  },
} satisfies Record<string, GlossaryEntry>;

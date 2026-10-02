import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Resource limits -----------------------------------------------------
  'memory-limit': {
    term: 'memory limit',
    definition: 'The most memory that one container can use. Above it, the kernel stops the process of the container.',
    see: ['oom', 'resource-limit'],
  },
  'cpu-limit': {
    term: 'CPU limit',
    definition: 'The most CPU time that one container can use. At the limit, the container becomes slower. It does not stop.',
    see: ['cpu-core', 'throttle'],
  },
  'cpu-core': {
    term: 'CPU',
    definition: 'The part of a machine that does the calculations. One CPU is one core. A server has one or more cores.',
    see: ['cpu-limit'],
  },
  millicore: {
    term: 'millicore',
    definition: 'One thousandth of a CPU core. `500m` is 500 millicores: half a core.',
    see: ['cpu-core'],
  },
  mib: {
    term: 'MiB',
    definition: 'A unit of memory and of disk space. 1 MiB is 1024 × 1024 bytes. 1 GiB is 1024 MiB.',
  },
  kernel: {
    term: 'kernel',
    definition: 'The center of the operating system. It gives memory and CPU time to each process, and it applies the limits.',
  },
  'os-process': {
    term: 'process',
    definition: 'One program that runs on a machine. An app can start many processes.',
    see: ['thread'],
  },
  thread: {
    term: 'thread',
    definition: 'One line of work in a process. A process can have many threads that work at the same time.',
    see: ['os-process'],
  },
  swap: {
    term: 'swap',
    definition: 'Disk space that a system uses as slow memory when the real memory is full.',
  },
  throttle: {
    term: 'throttle',
    definition: 'To make a container wait when it used all the CPU time of its limit. The app runs slower.',
    see: ['cpu-limit'],
  },
  'cpu-quota': {
    term: 'CPU quota',
    definition: 'The function of the Linux kernel that applies a CPU limit. Its full name is CFS quota.',
    see: ['cpu-limit', 'kernel'],
  },
  'pids-limit': {
    term: 'pids limit',
    definition: 'The largest number of processes and threads that one container can have at the same time.',
    see: ['os-process', 'fork-bomb'],
  },
  fork: {
    term: 'fork',
    definition: 'The system call that makes a new process from a process that runs.',
    see: ['os-process'],
  },
  'fork-bomb': {
    term: 'fork bomb',
    definition: 'A program that starts new processes with no end. It can fill a machine that has no limit.',
    see: ['pids-limit'],
  },
  'log-rotation': {
    term: 'log rotation',
    definition: 'When a log file is full, Docker starts a new file and deletes the oldest file. The log cannot fill the disk.',
    see: ['log-driver'],
  },
  'log-driver': {
    term: 'log driver',
    definition: 'The part of Docker that stores the output of containers. The default driver, `json-file`, writes files on the disk.',
  },
  'free-disk-check': {
    term: 'free-disk check',
    definition: 'The test that Ferry does before a build or a pull: the disk must have enough free space.',
  },
  'docker-host': {
    term: 'Docker host',
    definition: 'The machine that Docker runs on. Its CPUs and its memory are all that the containers can use.',
    see: ['docker'],
  },
  'oom-score': {
    term: 'OOM score',
    definition: 'A number for each process. When the machine has no free memory, the kernel first stops the process with the highest score.',
    see: ['oom', 'kernel'],
  },
  overcommit: {
    term: 'overcommit',
    definition: 'The limits of all containers together are larger than the memory of the server.',
    see: ['oom-score'],
  },
  maxmemory: {
    term: 'maxmemory',
    definition: 'The Redis setting for the most memory that Redis uses for data. When it is full, Redis refuses writes.',
  },
  signal: {
    term: 'signal',
    definition: 'A short message that the system sends to a process, for example to tell it to stop. `SIGINT` and `SIGTERM` ask a process to stop.',
    see: ['sigkill'],
  },
  sigkill: {
    term: 'SIGKILL',
    definition: 'The signal that stops a process immediately. The process cannot refuse it.',
    see: ['signal', 'exit-code'],
  },

  // --- Logs & events -------------------------------------------------------
  'deploy-log': {
    term: 'deploy log',
    definition: 'The log of one deploy: the build output and the steps that Ferry does.',
    see: ['log', 'deploy'],
  },
  'runtime-log': {
    term: 'runtime log',
    definition: 'What the containers of a service write while they run. Docker keeps it as long as the containers exist.',
    see: ['log', 'stdout', 'stderr'],
  },
  'job-log': {
    term: 'job log',
    definition: 'The output of one job run.',
    see: ['log', 'job'],
  },
  'server-log': {
    term: 'server log',
    definition: 'The log that `ferryd` writes about its own work: failed deploys, OOM kills, proxy warnings.',
    see: ['ferryd'],
  },
  timestamp: {
    term: 'timestamp',
    definition: 'The date and time when a line was written.',
  },
  'change-feed': {
    term: 'change feed',
    definition: 'One stream of events from the API. It tells each change on the server: what changed, not the new value.',
    see: ['event', 'sse'],
  },
  metric: {
    term: 'metric',
    definition: 'A number that measures a container at one moment, for example its CPU use or its memory use.',
  },

  // --- Self-healing --------------------------------------------------------
  'actual-state': {
    term: 'actual state',
    definition: 'What runs at this moment: the containers and the ports that Docker reports.',
    see: ['desired-state', 'reconciler'],
  },
  'reconciler-pass': {
    term: 'pass',
    definition: 'One run of the reconciler: it compares the two states one time and repairs the difference.',
    see: ['reconciler'],
  },
  orphan: {
    term: 'orphan',
    definition: 'A container of this server whose service or datastore does not exist. The reconciler removes it.',
  },
  'proxy-route': {
    term: 'route',
    definition: 'The link between a hostname and the instances of a service. The proxy uses it to send each request.',
    see: ['proxy'],
  },
  'route-watcher': {
    term: 'route watcher',
    definition: 'The loop in Ferry that lists the instances each second and updates the routes of the proxy.',
    see: ['proxy-route'],
  },
  'restart-policy': {
    term: 'restart policy',
    definition: 'The Docker rule that says what to do when a container stops. With `unless-stopped`, Docker starts the container again.',
  },
  'graceful-shutdown': {
    term: 'graceful shutdown',
    definition: 'A stop in order: the server completes the requests in progress, then closes each part, then exits.',
  },
  'name-prefix': {
    term: 'name prefix',
    definition: 'The start of the name of each Docker resource of one Ferry server. The default is `ferry`.',
  },
} satisfies Record<string, GlossaryEntry>;

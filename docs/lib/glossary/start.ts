import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Ferry and Render ----------------------------------------------------
  render: {
    term: 'Render',
    definition: 'A company that runs apps on its own servers. Ferry does the same work on a server that you control.',
  },
  'self-hosted': {
    term: 'self-hosted',
    definition: 'You run the program on a machine that you control, not on the machines of a company.',
  },
  'render-plan': {
    term: 'plan',
    definition: 'On Render, an instance type: a fixed quantity of memory and CPU. Ferry changes a plan into memory and CPU limits.',
    see: ['resource-limit'],
  },
  'pr-preview': {
    term: 'pull request preview',
    definition: 'A temporary copy of an app that a host makes for each pull request, to test the change.',
  },
  'secret-file': {
    term: 'secret file',
    definition: 'On Render, a file with secret text that the host puts in the container of a service.',
  },
  'ip-allow-list': {
    term: 'IP allow list',
    definition: 'A list of IP addresses. Only these addresses can connect.',
  },
  rbac: {
    term: 'role-based access control',
    definition: 'Each user has a role, and the role says what the user can do.',
  },

  // --- Build Ferry ---------------------------------------------------------
  rust: {
    term: 'Rust',
    definition: 'The programming language of Ferry.',
    see: ['cargo'],
  },
  cargo: {
    term: 'cargo',
    definition: 'The build tool of Rust. It makes the Ferry programs from the source code.',
  },
  npm: {
    term: 'npm',
    definition: 'The package tool of Node.js. It gets the libraries of a project and runs its build.',
  },
  'path-variable': {
    term: 'PATH',
    definition: 'The list of folders where the terminal looks for a program when you type its name.',
  },

  // --- Docker on your machine ----------------------------------------------
  'docker-daemon': {
    term: 'Docker daemon',
    definition: 'The part of Docker that stays in the background. It builds images and runs containers when a program asks.',
    see: ['docker-socket'],
  },
  'docker-socket': {
    term: 'Docker socket',
    definition: 'A special file on the machine. Programs talk to the Docker daemon through it.',
  },
  'docker-context': {
    term: 'Docker context',
    definition: 'A name in Docker for one daemon and its address. Colima and OrbStack each make a context.',
  },
  'docker-desktop': {
    term: 'Docker Desktop',
    definition: 'The Docker app for macOS and Windows. It runs the containers in a virtual machine.',
    see: ['virtual-machine'],
  },
  'virtual-machine': {
    term: 'virtual machine',
    definition: 'A computer that software makes inside a real computer. It has its own system, CPUs and memory.',
  },

  // --- The server ----------------------------------------------------------
  banner: {
    term: 'banner',
    definition: 'The lines that `ferryd` prints when it starts: its addresses, its limits and what to do next.',
  },
  'data-directory': {
    term: 'data directory',
    definition: 'The folder where `ferryd` keeps its state: the database, the logs, the uploads and the certificates. The default is `./ferry-data`.',
  },
  'admin-account': {
    term: 'account',
    definition: 'The one administrator of a Ferry server: an email and a password. You sign in to the dashboard with it.',
  },
  'setup-code': {
    term: 'setup code',
    definition: 'A secret code that `ferryd` makes while the server has no account. You need it to create the account.',
  },
  'server-token': {
    term: 'server token',
    definition: 'An API token that `ferryd` writes in its data directory at the first start. Scripts on the server use it.',
    see: ['api-token'],
  },
  background: {
    term: 'background',
    definition: 'A program in the background does not keep the terminal. You can close the terminal and the program continues.',
    see: ['foreground'],
  },
  foreground: {
    term: 'foreground',
    definition: 'A program in the foreground keeps the terminal and writes its output there until it stops.',
    see: ['background'],
  },
  'service-manager': {
    term: 'service manager',
    definition: 'A system program that starts other programs and starts them again after a crash: systemd on Linux, launchd on macOS.',
    see: ['systemd'],
  },
  'liveness-probe': {
    term: 'liveness probe',
    definition: 'A small request that tells you if a program is alive.',
  },
  'swagger-ui': {
    term: 'Swagger UI',
    definition: 'A web page that lists each endpoint of an API. You can try the endpoints from the page.',
  },
} satisfies Record<string, GlossaryEntry>;

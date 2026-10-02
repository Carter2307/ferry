import type { GlossaryEntry } from './types';

/** The terms that every section uses. */
export const terms = {
  // --- Ferry itself --------------------------------------------------------
  ferryd: {
    term: 'ferryd',
    definition: 'The Ferry server. It is one program that runs on your machine and controls Docker.',
    see: ['cli', 'dashboard'],
  },
  cli: {
    term: 'CLI',
    definition: 'Command-line interface: the `ferry` command. You type commands in a terminal and the CLI sends them to the server.',
    see: ['ferryd'],
  },
  dashboard: {
    term: 'dashboard',
    definition: 'The web pages of your Ferry server. You use them in a browser to create and control services.',
  },
  api: {
    term: 'API',
    definition: 'Application programming interface: the HTTP addresses that programs call to control Ferry. The CLI and the dashboard use the API.',
    see: ['api-token'],
  },
  'api-token': {
    term: 'API token',
    definition: 'A secret text that proves who you are to the API. Programs send it with each request.',
  },
  blueprint: {
    term: 'blueprint',
    definition: 'A YAML file (`ferry.yaml` or `render.yaml`) that describes your services and datastores. Ferry creates or updates them from the file.',
  },

  // --- What runs -----------------------------------------------------------
  service: {
    term: 'service',
    definition: 'One app that Ferry builds and runs for you. A service has a name, a type, a source and settings.',
    see: ['instance', 'deploy'],
  },
  'web-service': {
    term: 'web service',
    definition: 'A service that answers HTTP requests and has a public URL.',
  },
  'private-service': {
    term: 'private service',
    definition: 'A service with a port but no public URL. Only your other services can call it.',
    see: ['private-network'],
  },
  worker: {
    term: 'background worker',
    definition: 'A service that runs all the time and has no port. It does work that no request waits for.',
  },
  'cron-job': {
    term: 'cron job',
    definition: 'A service that runs a command on a schedule (for example each night), then stops.',
  },
  'static-site': {
    term: 'static site',
    definition: 'A service that is only files (HTML, CSS, JavaScript). Ferry serves the files with nginx.',
  },
  datastore: {
    term: 'datastore',
    definition: 'A Postgres or Redis database that Ferry runs for you, with its data on a volume.',
    see: ['volume'],
  },
  job: {
    term: 'job',
    definition: 'A command that runs one time in a new container of a service, then stops.',
  },
  instance: {
    term: 'instance',
    definition: 'One running container of a service. A service can have many instances that do the same work.',
    see: ['container'],
  },

  // --- Docker --------------------------------------------------------------
  docker: {
    term: 'Docker',
    definition: 'The program that builds images and runs containers. Ferry tells Docker what to do.',
    see: ['image', 'container'],
  },
  image: {
    term: 'image',
    definition: 'A package that contains your app and all that the app needs to run. Docker starts containers from an image.',
    see: ['container', 'build'],
  },
  container: {
    term: 'container',
    definition: 'A process that Docker starts from an image and keeps apart from the other processes of the machine.',
    see: ['image', 'instance'],
  },
  dockerfile: {
    term: 'Dockerfile',
    definition: 'A text file with the steps that build an image.',
    see: ['image'],
  },
  registry: {
    term: 'registry',
    definition: 'A server that stores images. Docker Hub is a registry.',
    see: ['image'],
  },
  volume: {
    term: 'volume',
    definition: 'A folder that Docker keeps on the server when a container is removed. Data on a volume stays.',
    see: ['disk'],
  },
  disk: {
    term: 'disk',
    definition: 'Storage for one service that stays between deploys. It is a Docker volume at a path that you choose.',
    see: ['volume'],
  },

  // --- Deploys -------------------------------------------------------------
  deploy: {
    term: 'deploy',
    definition: 'One release of a service. Ferry builds an image, starts new instances, tests them, then sends the traffic to them.',
    see: ['build', 'health-check'],
  },
  build: {
    term: 'build',
    definition: 'The step that makes an image from your code.',
    see: ['image', 'runtime'],
  },
  runtime: {
    term: 'runtime',
    definition: 'The language environment of your app (Node, Python, Go, Rust, Ruby, static). Ferry finds it from your files and writes the Dockerfile.',
  },
  'health-check': {
    term: 'health check',
    definition: 'The test that tells Ferry that a new instance can receive traffic.',
  },
  'zero-downtime': {
    term: 'zero downtime',
    definition: 'Your app answers during the full deploy. The old instances stop only after the new ones are ready.',
  },
  snapshot: {
    term: 'snapshot',
    definition: 'The exact image, variables, command and limits of a live deploy. Ferry saves them and uses them for each new container of that deploy.',
  },
  rollback: {
    term: 'rollback',
    definition: 'A deploy that uses the image of an earlier deploy again.',
  },
  restart: {
    term: 'restart',
    definition: 'A deploy that uses the live image again, with the current settings and variables. Nothing is built.',
  },
  trigger: {
    term: 'trigger',
    definition: 'The cause of a deploy: a command, a push to git, a change of variables, and so on.',
  },
  'deploy-hook': {
    term: 'deploy hook',
    definition: 'A secret URL. A request to this URL starts a deploy of one service.',
  },
  webhook: {
    term: 'webhook',
    definition: 'An HTTP request that one system sends to another when something occurs. GitHub sends a webhook to Ferry after each push.',
  },
  reconciler: {
    term: 'reconciler',
    definition: 'The loop in Ferry that compares what must run with what Docker runs, then repairs the difference.',
  },

  // --- Source --------------------------------------------------------------
  repository: {
    term: 'repository',
    definition: 'A folder of code with its history, kept by git. Also called a repo.',
    see: ['branch', 'commit'],
  },
  branch: {
    term: 'branch',
    definition: 'One line of work in a git repository. Ferry deploys the newest commit of the branch that you choose.',
  },
  commit: {
    term: 'commit',
    definition: 'One saved version of the code in git. A commit has an identifier named SHA.',
  },

  // --- Configuration -------------------------------------------------------
  'env-var': {
    term: 'environment variable',
    definition: 'A value with a name that Ferry gives to your app when it starts. Example: `DATABASE_URL`.',
    see: ['env-group', 'reference'],
  },
  'env-group': {
    term: 'env group',
    definition: 'A set of environment variables with a name. You link it to all the services that need the same values.',
  },
  reference: {
    term: 'reference',
    definition: 'A value written as `${{…}}`. Ferry replaces it with the real value (for example a database URL) when the service starts.',
  },
  'resource-limit': {
    term: 'resource limit',
    definition: 'The most memory and CPU that one container can use.',
    see: ['oom'],
  },
  oom: {
    term: 'OOM kill',
    definition: 'Out of memory: the system stops a container that uses more memory than its limit.',
  },

  // --- Network -------------------------------------------------------------
  proxy: {
    term: 'proxy',
    definition: 'The part of Ferry that receives each HTTP request and sends it to an instance of the correct service.',
    see: ['load-balancing'],
  },
  'load-balancing': {
    term: 'load balancing',
    definition: 'The proxy gives each new request to the next instance, in turn. All instances share the work.',
  },
  port: {
    term: 'port',
    definition: 'A number that identifies one program on a machine for network connections. Your app listens on a port.',
  },
  'private-network': {
    term: 'private network',
    definition: 'A Docker network that only your services and datastores are on. They call each other by name, for example `api:3000`.',
  },
  hostname: {
    term: 'hostname',
    definition: 'The name of a machine or a site on a network, for example `api.example.com`.',
  },
  domain: {
    term: 'domain',
    definition: 'A name that you own on the internet, for example `example.com`.',
    see: ['dns'],
  },
  dns: {
    term: 'DNS',
    definition: 'Domain Name System: the directory of the internet. It gives the IP address of a server for a domain name.',
    see: ['dns-record'],
  },
  'dns-record': {
    term: 'DNS record',
    definition: 'One line in the DNS of your domain. An A record gives an IP address. A CNAME record points to another name.',
  },
  https: {
    term: 'HTTPS',
    definition: 'HTTP with encryption. The browser and the server use a certificate to keep the traffic private.',
    see: ['certificate'],
  },
  certificate: {
    term: 'certificate',
    definition: 'A file that proves that a server owns a domain. HTTPS needs one. Ferry gets certificates from Let’s Encrypt.',
    see: ['lets-encrypt'],
  },
  'lets-encrypt': {
    term: 'Let’s Encrypt',
    definition: 'A free service that gives HTTPS certificates. It first tests that your server controls the domain.',
  },
  localhost: {
    term: 'localhost',
    definition: 'The name of your own machine. `127.0.0.1` is its address. Other machines cannot connect to it.',
  },

  // --- Operations ----------------------------------------------------------
  log: {
    term: 'log',
    definition: 'The lines of text that a program writes while it runs. You read them to know what occurred.',
  },
  event: {
    term: 'event',
    definition: 'A record of something that occurred on the server, for example "deploy live" or "instance crashed".',
  },
  systemd: {
    term: 'systemd',
    definition: 'The Linux program that starts services when the machine boots and starts them again after a crash.',
  },
  sqlite: {
    term: 'SQLite',
    definition: 'A small database that is one file. Ferry keeps its own data (services, deploys, settings) in it.',
  },
  'desired-state': {
    term: 'desired state',
    definition: 'What must run, as written in the Ferry database: services, live deploys, number of instances.',
    see: ['reconciler'],
  },
} satisfies Record<string, GlossaryEntry>;

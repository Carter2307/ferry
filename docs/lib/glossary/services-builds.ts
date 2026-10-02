import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Services ------------------------------------------------------------
  source: {
    term: 'source',
    definition: 'Where the code of a service comes from: a git repository, a Docker image or an upload.',
    see: ['repository', 'image', 'upload'],
  },
  'service-type': {
    term: 'service type',
    definition: 'The kind of a service: web service, private service, background worker, cron job or static site.',
    see: ['service'],
  },
  nginx: {
    term: 'nginx',
    definition: 'A web server program. Ferry uses it to send the files of a static site to the browser.',
    see: ['static-site'],
  },
  upload: {
    term: 'upload',
    definition: 'A folder of code that `ferry up` sends from your machine to the server.',
    see: ['archive'],
  },
  archive: {
    term: 'archive',
    definition: 'One file that contains many files and folders. A `.tar.gz` archive is also compressed.',
  },
  'ignore-file': {
    term: 'ignore file',
    definition: 'A text file that lists the files to leave out: `.gitignore`, `.ignore`, `.ferryignore` or `.dockerignore`.',
  },
  symlink: {
    term: 'symlink',
    definition: 'A file that points to another file or folder.',
  },
  submodule: {
    term: 'git submodule',
    definition: 'A git repository that is a folder of another git repository.',
    see: ['repository'],
  },
  'git-cache': {
    term: 'git cache',
    definition: 'The copy of the repository that Ferry keeps on the server for one service. The next deploy gets only the new commits.',
    see: ['repository'],
  },
  'auto-deploy': {
    term: 'auto-deploy',
    definition: 'A setting of a service. When it is on, a push to the branch starts a deploy.',
    see: ['webhook'],
  },
  'image-tag': {
    term: 'tag',
    definition: 'The part of an image name after the `:`. It names one version of the image, for example `alpine` in `nginx:alpine`.',
    see: ['image'],
  },
  'pinned-image': {
    term: 'pinned image',
    definition: 'An image that Ferry keeps with a tag of its own for one deploy. This deploy always runs the same image.',
    see: ['image-tag'],
  },
  'cron-schedule': {
    term: 'schedule',
    definition: 'The times at which a cron job runs. You write it as a cron expression, for example `0 3 * * *`.',
    see: ['cron-job', 'cron-expression'],
  },
  utc: {
    term: 'UTC',
    definition: 'Coordinated Universal Time: the reference time of the world. It has no summer time.',
  },
  'service-state': {
    term: 'state',
    definition: 'What a service does now: `live`, `deploying`, `failed`, and so on. Ferry computes it from the deploys and the containers.',
    see: ['service'],
  },
  'internal-address': {
    term: 'internal address',
    definition: 'The name and the port of a service on the private network, for example `api:3000`.',
    see: ['private-network'],
  },
  'service-id': {
    term: 'service id',
    definition: 'The identifier that Ferry gives to a service: `srv-` and 20 hex digits. Commands accept the id or the name.',
  },

  // --- Builds --------------------------------------------------------------
  monorepo: {
    term: 'monorepo',
    definition: 'One repository that contains many apps, each in its own folder.',
    see: ['root-directory'],
  },
  'root-directory': {
    term: 'root directory',
    definition: 'The folder of your code where Ferry builds the service. The default is the top of the repository.',
    see: ['monorepo'],
  },
  'publish-directory': {
    term: 'publish directory',
    definition: 'The folder of a static site that nginx serves, for example `dist`.',
    see: ['static-site'],
  },
  'build-command': {
    term: 'build command',
    definition: 'A command that prepares your app during the build, for example `npm run build`.',
    see: ['build'],
  },
  'start-command': {
    term: 'start command',
    definition: 'The command that starts your app in each container, for example `npm start`.',
    see: ['procfile'],
  },
  procfile: {
    term: 'Procfile',
    definition: 'A text file named `Procfile`. Each line has a name and a start command, for example `web: python app.py`.',
    see: ['start-command'],
  },
  'base-image': {
    term: 'base image',
    definition: 'The image that a Dockerfile starts from. It contains a small system and the tools of one language.',
    see: ['dockerfile'],
  },
  layer: {
    term: 'layer',
    definition: 'The result of one step of a Dockerfile. An image is a stack of layers.',
    see: ['build-cache'],
  },
  'build-cache': {
    term: 'build cache',
    definition: 'The layers that Docker keeps from earlier builds. Docker uses a layer again when its step did not change.',
    see: ['layer'],
  },
  buildkit: {
    term: 'BuildKit',
    definition: 'The build engine of Docker. It runs the steps of a Dockerfile and keeps the cache.',
    see: ['docker', 'build'],
  },
  'build-secret': {
    term: 'build secret',
    definition: 'A value that Docker gives to one build step only. Docker does not record it in the image.',
    see: ['build-argument'],
  },
  'build-argument': {
    term: 'build argument',
    definition: 'A value that `docker build --build-arg` gives to a Dockerfile. Docker records it in the history of the image.',
    see: ['build-secret'],
  },
  'image-history': {
    term: 'image history',
    definition: 'The list of the steps that made an image. Each person who has the image can read it.',
    see: ['image'],
  },
  dependency: {
    term: 'dependency',
    definition: 'A library that your app needs. A file such as `package.json` or `requirements.txt` lists the dependencies.',
    see: ['package-manager'],
  },
  'package-manager': {
    term: 'package manager',
    definition: 'The tool that installs the dependencies of a project: npm, Yarn, pnpm, Bun, pip, Bundler, and so on.',
    see: ['dependency'],
  },
  lockfile: {
    term: 'lockfile',
    definition: 'A file that records the exact version of each dependency, for example `package-lock.json`.',
    see: ['dependency'],
  },
  toolchain: {
    term: 'toolchain',
    definition: 'The tools that build a project in one language, for example Node.js with npm.',
  },
  'cpu-architecture': {
    term: 'CPU architecture',
    definition: 'The family of the processor of a machine, for example x86-64 or ARM64. An image runs on one architecture.',
  },
} satisfies Record<string, GlossaryEntry>;

import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Linux ---------------------------------------------------------------
  ssh: {
    term: 'SSH',
    definition: 'Secure Shell: the tool that opens a terminal on another machine through an encrypted connection. It uses port 22.',
    see: ['ssh-tunnel'],
  },
  root: {
    term: 'root',
    definition: 'The administrator account of a Linux machine. It can read, change and delete all things.',
    see: ['sudo'],
  },
  sudo: {
    term: 'sudo',
    definition: 'A Linux command that runs one other command as root.',
    see: ['root'],
  },
  'system-user': {
    term: 'system user',
    definition: 'A Linux account for a program, not for a person. No person can log in with it.',
    see: ['linux-group'],
  },
  'linux-group': {
    term: 'group',
    definition: 'A set of Linux users. A right that the group has applies to each of its members.',
  },
  'file-mode': {
    term: 'file mode',
    definition: 'The digits that tell who can read, write or run a file. With `600` and `700`, only the owner has access.',
  },
  cgroup: {
    term: 'cgroup',
    definition: 'Control group: the Linux function that measures and limits the memory and CPU of a set of processes. Docker makes one for each container.',
    see: ['kernel'],
  },

  // --- systemd -------------------------------------------------------------
  'systemd-unit': {
    term: 'unit',
    definition: 'A file that tells systemd how to start, stop and supervise one program.',
    see: ['systemd'],
  },
  'env-file': {
    term: 'environment file',
    definition: 'A text file with one `NAME=value` on each line. systemd gives these values to the program as environment variables.',
    see: ['env-var'],
  },
  journal: {
    term: 'journal',
    definition: 'The place where systemd keeps the output of the programs that it runs. The command `journalctl` reads it.',
    see: ['systemd'],
  },

  // --- Network -------------------------------------------------------------
  'reverse-proxy': {
    term: 'reverse proxy',
    definition: 'A program that receives the requests from the internet and sends them to another program. nginx, Caddy and Traefik are reverse proxies.',
    see: ['proxy'],
  },
  'load-balancer': {
    term: 'load balancer',
    definition: 'A machine or a cloud product that receives the traffic and gives it to one or more servers.',
  },
  'wildcard-certificate': {
    term: 'wildcard certificate',
    definition: 'One certificate for `*.example.com`. It is good for each name directly under the domain.',
    see: ['certificate'],
  },

  // --- Ferry servers -------------------------------------------------------
  'docker-root': {
    term: 'Docker root directory',
    definition: 'The folder where Docker keeps its images, containers and volumes.',
  },
  'rootless-docker': {
    term: 'rootless Docker',
    definition: 'A Docker daemon that runs as a normal user, not as root.',
  },
  'marker-volume': {
    term: 'marker volume',
    definition: 'The Docker volume `<prefix>-owner`. It records which data directory owns the name prefix.',
    see: ['instance-id'],
  },
  'instance-id': {
    term: 'instance_id',
    definition: 'The file of the data directory that holds the identity of one Ferry server.',
  },
  'data-dir-lock': {
    term: 'lock',
    definition: 'A mark on a file that one program holds. It tells other programs that the folder is in use.',
  },
  staging: {
    term: 'staging',
    definition: 'A second copy of your setup where you test a change before it goes to production.',
  },

  // --- The docs site -------------------------------------------------------
  'static-export': {
    term: 'static export',
    definition: 'A full site written as plain files (HTML, CSS, JavaScript, images). No program runs on the server to make the pages.',
    see: ['static-site'],
  },
  'open-graph': {
    term: 'Open Graph',
    definition: 'Data in a page that gives the title and the image that show when a person shares the link.',
  },
  'canonical-url': {
    term: 'canonical URL',
    definition: 'The official address of a page. It tells search engines which URL to show.',
  },
  'dev-dependencies': {
    term: 'devDependencies',
    definition: 'The packages of `package.json` that only the build needs, for example the build tools.',
  },
} satisfies Record<string, GlossaryEntry>;

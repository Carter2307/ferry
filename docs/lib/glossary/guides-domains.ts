import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Domains -------------------------------------------------------------
  subdomain: {
    term: 'subdomain',
    definition: 'A name under a domain. `shop.example.com` is a subdomain of `example.com`.',
    see: ['domain'],
  },
  apex: {
    term: 'apex domain',
    definition: 'The domain itself, with no name before it: `example.com`. DNS providers write it as `@`.',
    see: ['subdomain'],
  },
  registrar: {
    term: 'registrar',
    definition: 'A company that sells domain names.',
    see: ['dns-provider'],
  },
  'dns-provider': {
    term: 'DNS provider',
    definition: 'The company where you manage the DNS records of your domain. Frequently, it is the company that sold you the domain.',
    see: ['dns-record'],
  },
  'dns-zone': {
    term: 'DNS zone',
    definition: 'All the DNS records of one domain at your DNS provider.',
    see: ['dns-provider'],
  },
  'dns-cache': {
    term: 'DNS cache',
    definition: 'The memory of a DNS server. It keeps an answer for some time and gives it again without a new search.',
    see: ['dns'],
  },
  resolve: {
    term: 'resolve',
    definition: 'A name resolves to an address when the DNS gives this address for the name.',
    see: ['dns'],
  },
  'private-address': {
    term: 'private address',
    definition: 'An IP address that works only in one local network, for example `192.168.1.10`. Machines on the internet cannot reach it.',
    see: ['public-address', 'nat'],
  },
  'network-interface': {
    term: 'network interface',
    definition: 'The part of a machine that connects it to a network. Each network interface has an IP address.',
    see: ['ip-address'],
  },
  'public-address': {
    term: 'public address',
    definition: 'The IP address that machines on the internet use to reach your server.',
    see: ['ip-address'],
  },
  ipv6: {
    term: 'IPv6',
    definition: 'The newer format of IP addresses, for example `2001:db8::10`. An `AAAA` record gives an IPv6 address. An `A` record gives an IPv4 address.',
    see: ['ip-address', 'dns-record'],
  },
  nat: {
    term: 'NAT',
    definition: 'Network address translation: a router gives one public address to machines that have private addresses.',
    see: ['public-address'],
  },
  firewall: {
    term: 'firewall',
    definition: 'A filter that blocks the network connections to a machine, but not on the ports that you open.',
    see: ['port'],
  },
  'rate-limit': {
    term: 'rate limit',
    definition: 'The largest number of requests that a service accepts in a period of time.',
  },

  // --- Migrate from Render -------------------------------------------------
  'key-value-store': {
    term: 'key value store',
    definition: 'The name that Render gives to a Redis database. On Ferry, it is a Redis datastore.',
    see: ['datastore'],
  },
  'pg-dump': {
    term: 'pg_dump',
    definition: 'The Postgres tool that writes a full database into one file. The tool `psql` loads the file into another database.',
  },

  // --- Troubleshooting -----------------------------------------------------
  capability: {
    term: 'capability',
    definition: 'One special right that Linux gives to a program. `CAP_NET_BIND_SERVICE` lets a program listen on the ports below 1024.',
  },
  'password-hash': {
    term: 'hash',
    definition: 'A text that a program calculates from a password. The program can compare it, but nobody can read the password from it.',
  },
  stdin: {
    term: 'standard input',
    definition: 'The channel where a program reads its input. A pipe (`|`) or a file can give the input, not only the keyboard.',
  },
  'build-context': {
    term: 'build context',
    definition: 'The folder of files that Ferry gives to the build: the repository, or its root directory.',
    see: ['build'],
  },
  'ferry-error-header': {
    term: 'x-ferry-error',
    definition: 'A header that the proxy adds when it answers with its own error page. Your app does not send it.',
    see: ['proxy'],
  },
} satisfies Record<string, GlossaryEntry>;

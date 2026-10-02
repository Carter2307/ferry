import type { GlossaryEntry } from './types';

/** Terms of this section. An id must not exist in another file of this folder. */
export const terms = {
  // --- Requests and the proxy ----------------------------------------------
  'ip-address': {
    term: 'IP address',
    definition: 'The number of a machine on a network, for example `203.0.113.10`. Machines use it to find each other.',
    see: ['dns'],
  },
  'public-url': {
    term: 'public URL',
    definition: 'The address that a browser on the internet uses to open your service, for example `https://shop.example.com`.',
    see: ['hostname'],
  },
  header: {
    term: 'header',
    definition: 'One line of an HTTP request or answer that has a name and a value, for example `Host: shop.example.com`.',
    see: ['host-header'],
  },
  'host-header': {
    term: 'Host header',
    definition: 'The header of a request that gives the hostname that the client wants. The proxy reads it to find the service.',
    see: ['header', 'hostname'],
  },
  'status-code': {
    term: 'status code',
    definition: 'The number at the start of an HTTP answer. `200` is a success, `404` is "not found", `503` is "not available".',
  },
  redirect: {
    term: 'redirect',
    definition: 'An answer that tells the browser to go to another URL. Ferry uses the status code `308` to send HTTP to HTTPS.',
    see: ['status-code'],
  },
  'round-robin': {
    term: 'round-robin',
    definition: 'A rule of turns: the first request goes to the first instance, the next request to the next instance, then the list starts again.',
    see: ['load-balancing'],
  },
  tcp: {
    term: 'TCP connection',
    definition: 'The basic link between two programs on a network. HTTP sends its requests through a TCP connection.',
    see: ['port'],
  },
  timeout: {
    term: 'timeout',
    definition: 'The longest time that a program waits for something. After this time, the program stops the wait.',
  },
  'open-file-limit': {
    term: 'open-file limit',
    definition: 'The number of files and connections that the system lets one program keep open at the same time.',
  },
  websocket: {
    term: 'WebSocket',
    definition: 'A connection that stays open between a browser and an app. The two sides send messages at any time.',
  },
  sse: {
    term: 'Server-Sent Events',
    definition: 'SSE: one HTTP answer that stays open. The app sends new lines of data to the browser when it has them.',
  },
  http2: {
    term: 'HTTP/2',
    definition: 'A newer version of HTTP. It sends many requests at the same time through one connection.',
  },
  grpc: {
    term: 'gRPC',
    definition: 'A protocol that programs use to call each other. It needs HTTP/2 from one end to the other.',
    see: ['http2'],
  },
  cdn: {
    term: 'CDN',
    definition: 'Content delivery network: servers of a company (for example Cloudflare) that receive the requests first and send them to your server.',
  },

  // --- The private network -------------------------------------------------
  'bridge-network': {
    term: 'bridge network',
    definition: 'A network that Docker makes inside one machine. The containers on it can call each other. Other machines cannot reach it.',
    see: ['private-network'],
  },
  alias: {
    term: 'alias',
    definition: 'A name that Docker gives to a container on a network. Other containers use this name, not an IP address.',
    see: ['private-network'],
  },
  'ssh-tunnel': {
    term: 'SSH tunnel',
    definition: 'A safe connection through SSH that makes a port of the server available as a port of your machine.',
  },

  // --- Domains -------------------------------------------------------------
  'base-domain': {
    term: 'base domain',
    definition: 'The first domain of a Ferry server. It comes from `ferryd --base-domain`. Its default is `localhost`.',
    see: ['server-domain'],
  },
  'server-domain': {
    term: 'domain of the server',
    definition: 'A domain that Ferry serves all public services under. With `example.com`, a service named `shop` answers at `shop.example.com`.',
    see: ['base-domain', 'custom-domain'],
  },
  'default-domain': {
    term: 'default domain',
    definition: 'The domain of the server that Ferry uses when it shows the URL of a service.',
    see: ['server-domain'],
  },
  'custom-domain': {
    term: 'custom domain',
    definition: 'One more hostname that you add to one service, for example `www.example.com`.',
    see: ['server-domain'],
  },
  'wildcard-record': {
    term: 'wildcard DNS record',
    definition: 'A DNS record for `*.example.com`. It answers for each name under the domain, thus a new service needs no new record.',
    see: ['dns-record'],
  },
  'local-name': {
    term: 'local name',
    definition: 'A name that has no public DNS: `localhost`, `*.localhost`, `*.local`, `*.internal` and `*.test`. It gets no certificate.',
    see: ['localhost'],
  },

  // --- HTTPS ---------------------------------------------------------------
  tls: {
    term: 'TLS',
    definition: 'Transport Layer Security: the encryption that HTTPS uses between the browser and the server.',
    see: ['https', 'certificate'],
  },
  'certificate-authority': {
    term: 'certificate authority',
    definition: 'CA: an organization that browsers trust and that gives certificates. Let’s Encrypt is a certificate authority.',
    see: ['lets-encrypt'],
  },
  acme: {
    term: 'ACME',
    definition: 'The protocol that a server uses to ask a certificate authority for a certificate, with no person in the middle.',
    see: ['certificate-authority', 'http-01'],
  },
  'http-01': {
    term: 'HTTP-01 challenge',
    definition: 'The test of ACME: the certificate authority asks the host for a secret file on port 80. A correct answer proves control of the hostname.',
    see: ['acme'],
  },
  'self-signed': {
    term: 'self-signed certificate',
    definition: 'A certificate that a server makes for itself. No certificate authority signed it, thus browsers show a warning.',
    see: ['certificate'],
  },

  // --- Environment ---------------------------------------------------------
  'injected-variable': {
    term: 'injected variable',
    definition: 'An environment variable that Ferry adds to each container, for example `PORT` or `FERRY_SERVICE_NAME`.',
    see: ['env-var'],
  },
  'merged-env': {
    term: 'merged environment',
    definition: 'All the variables that a service gets: the injected variables, then its env groups, then its own variables.',
    see: ['precedence'],
  },
  precedence: {
    term: 'precedence',
    definition: 'The rule that tells which value wins when the same key exists in two places.',
    see: ['merged-env'],
  },
  shell: {
    term: 'shell',
    definition: 'The program that reads the commands that you type in a terminal, for example bash or zsh.',
  },
  kib: {
    term: 'KiB',
    definition: 'Kibibyte: 1024 bytes. 32 KiB is 32,768 bytes.',
  },
  'connection-string': {
    term: 'connection string',
    definition: 'One URL that contains all that an app needs to connect to a database: user, password, host, port and database name.',
    see: ['datastore'],
  },
} satisfies Record<string, GlossaryEntry>;

# Ferry ⛴

**A self-hosted Render alternative, written in Rust.**

Ferry turns one machine with Docker into your own Render. You point it at a git
repo, a local folder or a Docker image, and it builds the code, deploys it with
zero downtime and gives it a URL. It also scales, heals and logs your apps, and
runs managed Postgres/Redis, cron jobs, env groups, custom domains with
automatic HTTPS, and `render.yaml` blueprints.

```
ferry up                      # deploy the current directory → http://my-app.localhost:8080
```

## Features

| | |
|---|---|
| **Service types** | web services, private services, background workers, cron jobs, static sites |
| **Sources** | any git URL (GitHub, GitLab, ssh, local path), `ferry up` from a local folder, prebuilt images |
| **Builds** | your Dockerfile, or auto-detected Node / Python / Go / Rust / Ruby / static (no Dockerfile needed) |
| **Deploys** | zero-downtime blue/green with health checks, deploy history, one-click rollbacks, cancel |
| **Triggers** | CLI / dashboard / API, GitHub push webhooks (auto-deploy), secret deploy-hook URLs, blueprints |
| **Networking** | built-in reverse proxy (HTTP/1.1, HTTP/2, websockets), `<service>.<your-domain>` under the domains you connect (Ferry checks their DNS), custom domains, Let's Encrypt HTTPS, private network (`http://api:3000`) |
| **Data** | managed Postgres & Redis, persistent disks, env vars with references (`${{datastore.db.connectionString}}`), env groups |
| **Ops** | instance scaling, suspend/resume, restart, self-healing reconciler, live build & runtime logs, CPU/memory, one-off jobs |
| **Guardrails** | memory/CPU limits per service and datastore (server defaults 512 MiB / 1 CPU), pids limit and log rotation on every container, out-of-memory kills reported, free-disk check before deploys |
| **Interfaces** | web dashboard, `ferry` CLI, REST API, `ferry.yaml` / `render.yaml` blueprints |

## Quick start

Requirements: Docker (Docker Desktop on macOS works), Rust 1.89+ and Node.js 20+ to build.

```bash
(cd web && npm ci && npm run build)     # the web dashboard (embedded into ferryd at compile time)
cargo build --release
./target/release/ferryd                 # starts the server in the background; prints the dashboard URL and the link that creates your account
```

`ferryd` gives the terminal back once the server listens, and the server keeps
running when you close it. `ferryd status` says where it listens, `ferryd logs -f`
follows its log (`ferry-data/ferryd.log`), `ferryd stop` shuts it down, and
`ferryd run` keeps it in the foreground instead (see [Running the server](#running-the-server)).

Open the link `ferryd` printed (`http://127.0.0.1:7878/setup?code=…`) and create your
account: the email and password you sign in to the dashboard with. Then connect the CLI
and deploy:

```bash
./target/release/ferry login             # opens the dashboard: approve this terminal there

cd examples/node-hello
ferry up --follow                        # creates "node-hello", uploads, builds, deploys
curl http://node-hello.localhost:8080
```

The dashboard is at **http://127.0.0.1:7878** (or http://ferry.localhost:8080). The interactive
API reference (Swagger UI) is at **http://127.0.0.1:7878/api/docs**, and the OpenAPI 3.1 document is at `/api/openapi.json`.

### More examples

```bash
# From git, with 2 instances, a health check and 1 GiB / 2 CPUs per instance
ferry create api --repo https://github.com/you/api --branch main --instances 2 --health /healthz \
  --memory 1G --cpu 2 --follow

# Prebuilt image
ferry create hello --image nginx:alpine --port 80

# Postgres, wired in through an env var reference
ferry db create app-db --kind postgres
ferry env set api 'DATABASE_URL=${{datastore.app-db.connectionString}}'

# Cron job
ferry create nightly --type cron --image busybox:stable --schedule "0 3 * * *" --start-cmd 'echo hello'
ferry run nightly --follow               # run it now

# Everything as code (render.yaml works too)
ferry blueprint apply examples/blueprint/ferry.yaml

# Day-2
ferry logs api -f
ferry scale api 3
ferry rollback api <deploy-id>
ferry domains connect example.com        # every service at <service>.example.com (see Domains below)
ferry domains add api api.example.org    # one more name for one service
ferry update api --memory 2G && ferry restart api   # new limits apply with the next deploy or restart
ferry db update app-db --memory 1G                   # datastores: applied right away, no restart
```

## Production setup

1. A Linux VM with Docker, reachable on ports 80 and 443. Point a wildcard DNS
   record `*.apps.example.com` (and your custom domains) at it — or start
   without a domain and [connect one](#domains) from the dashboard.
2. Run `ferryd` as a service (systemd), with `ferryd run`, which stays in
   the foreground for the service manager to supervise. For example:

   ```ini
   [Service]
   ExecStart=/usr/local/bin/ferryd run \
     --data-dir /var/lib/ferry \
     --base-domain apps.example.com \
     --proxy-addr 0.0.0.0:80 --https-addr 0.0.0.0:443 \
     --acme-email you@example.com \
     --github-webhook-secret <random>
   Restart=always
   # When the host runs out of memory, let the kernel kill containers before ferryd.
   # (ferryd also sets -500 itself when it runs as root; see --oom-score-adj.)
   OOMScoreAdjust=-900
   ```

   Only ferryd keeps this score: the processes it starts (`git`, the
   `docker` CLI) are reset to 0, so a runaway `git` isn't spared at the
   expense of your apps and databases. Containers (and builds, which run
   inside Docker) live in Docker's cgroups, not in this unit, so memory
   settings here only cover ferryd and the processes it starts. A
   `MemoryMin=` here does nothing unless its parent slice (`system.slice`)
   reserves memory too. Avoid a `MemoryMax=` on this unit: reaching it gets
   ferryd itself killed. Limit the apps with
   `--default-memory-limit` or per service instead (see
   [Resource limits](#resource-limits)).

3. Keep the API on localhost and reach it over SSH (`ssh -L 7878:127.0.0.1:7878 host`).
   You can also expose the dashboard through the proxy with
   `--dashboard-host ferry.apps.example.com`, but only with HTTPS enabled,
   because it serves the full admin API.
4. For GitHub auto-deploys, add a webhook to your repo:
   `https://ferry.apps.example.com/hooks/github`, content type JSON, with the
   same secret.

### Domains

Every web service and static site is served at `<service>.<domain>`, for
each domain of the server. A server starts with one domain, its
`--base-domain` (`localhost` by default), and you connect others while it
runs — no restart, no deploy:

1. **Connect it**: **Server → Domains → Connect a domain** in the
   dashboard, or `ferry domains connect example.com`. A subdomain works too
   (`apps.example.com`).
2. **Create one DNS record** where the DNS of the domain is managed (your
   registrar or DNS provider). Ferry shows it with the server's public
   address:

   | Type | Name | Value |
   |---|---|---|
   | `A` | `*` | the public IP address of the server |

   `*` is a wildcard: one record answers for every name under the domain, so
   a new service needs no new record. A second record, `A` `@`, is only
   needed to serve the domain itself.
3. **Ferry verifies it**: every 15 seconds it resolves a made-up name under
   the domain and checks that the request comes back to this very server.
   Once it does, the domain is `active`: every service is served under it,
   gets its certificate within seconds when HTTPS is on, and the domain
   becomes the default one — the one service URLs are shown with — if the
   default was a local name. Until then the dashboard and `ferry domains`
   say what is wrong: no record yet, a record that points elsewhere, another
   server answering, a closed port.

```bash
ferry domains                          # the domains, their status, the record to create
ferry domains verify example.com       # check now (exit 1 while it doesn't reach the server)
ferry domains default example.com      # the domain service URLs are shown with
ferry domains disconnect example.com
ferry certificates                     # every hostname with the state of its certificate
```

A domain whose DNS breaks later is flagged `misconfigured` after three
failed checks, and stays served. The base domain is always served and is
only changed with `--base-domain`. Local names (`*.localhost`, `.test`…)
need no DNS and are never verified.

Ferry finds the server's public IPv4 address by itself: from its network
interface, or — behind NAT — by asking a public service which address it
sees (api.ipify.org, then ipv4.icanhazip.com, then checkip.amazonaws.com). Set
`--public-ip` to say it yourself (also the only way to get `AAAA` records
for IPv6); nothing is asked then.

HTTPS is still turned on when the server starts (`--https-addr` and
`--acme-email`), and the dashboard is served through the proxy at the host
given by `--dashboard-host`, not under each domain. A public server started
without `--base-domain` should also get `--dashboard-host none` (or a
hostname of its own, with HTTPS): with the default `localhost` base domain,
the proxy answers the dashboard for the host `ferry.localhost`.

### Running the server

| Command | What it does |
|---|---|
| `ferryd [options]` | `ferryd start` when run from a terminal, `ferryd run` otherwise (systemd, a container, a pipe) |
| `ferryd start [options]` | starts the server in the background and returns once it listens; a start that fails prints why and exits 1. The server's output goes to `<data-dir>/ferryd.log` |
| `ferryd run [options]` | runs the server in the foreground until Ctrl-C or `SIGTERM`: for service managers, containers, or to watch the log |
| `ferryd status` | where the server listens; exit code 0 when it runs, 3 when it doesn't |
| `ferryd logs [-f] [-n N]` | the end of the log of a server started in the background (`-f` follows it) |
| `ferryd stop` | asks the server to shut down and waits until it has (works for a foreground server too); a second `ferryd stop` forces it, like a second Ctrl-C |
| `ferryd reset-password` | replaces the password of the server's account (asked without echo, or read from standard input) and signs every browser out |

`stop`, `status` and `logs` find the server through its data directory: run
them where you started `ferryd`, or with the same `--data-dir` /
`FERRY_DATA_DIR`. Apps and datastores keep running in Docker while `ferryd`
is stopped; their public URLs answer again once it is back.

### Account, sessions and API tokens

A server has one account, its administrator. `ferryd` prints a link
(`/setup?code=…`) while it has none; the code in it is also in
`<data-dir>/setup_code`, so only someone on the server can create the account.

| Who | Authenticates with |
|---|---|
| The dashboard | the account's email and password. The session is an `HttpOnly`, `SameSite=Strict` cookie that ends 30 days after its last use; a request that changes something must also come from the dashboard's own origin |
| A terminal | `ferry login --server URL`: the dashboard shows who asks and a code, you approve, and the terminal gets an API token of its own |
| Scripts and CI | an API token created under **Server → Account** (named, optionally expiring, shown once): `FERRY_TOKEN`, or `Authorization: Bearer <token>` |
| Scripts on the server itself | the server token in `<data-dir>/api_token` (`0600`), which always works and is never printed |

Tokens and sessions are stored as SHA-256 digests, the password as an Argon2id
hash. API tokens are revoked in the dashboard; they can do everything in the
API except change the account, its sessions and its tokens. Ten failed
sign-ins in five minutes pause sign-ins for the whole server. A forgotten
password is replaced on the server with `ferryd reset-password`.

### Server configuration

Every flag also has an environment variable. `ferryd --help` shows the full list.

| Flag / env | Default | Meaning |
|---|---|---|
| `--data-dir` `FERRY_DATA_DIR` | `./ferry-data` | database, logs, build scratch, certs (created `0700`, since it holds secrets) |
| `--api-addr` `FERRY_API_ADDR` | `127.0.0.1:7878` | API + dashboard |
| `--proxy-addr` `FERRY_PROXY_ADDR` | `0.0.0.0:8080` | public HTTP |
| `--https-addr` `FERRY_HTTPS_ADDR` | – | public HTTPS (together with `--acme-email`) |
| `--base-domain` `FERRY_BASE_DOMAIN` | `localhost` | the domain the server starts with: apps live at `<name>.<base-domain>`. More are [connected](#domains) while it runs |
| `--public-ip` `FERRY_PUBLIC_IP` | detected (IPv4) | the address(es) the server is reached at from the internet: what the DNS records of a domain point at, and what its DNS is checked against. Repeat the flag (or separate with commas) for IPv4 + IPv6 |
| `--public-port` `FERRY_PUBLIC_PORT` | proxy port | port shown in app URLs (e.g. behind port forwarding) |
| `--dashboard-host` `FERRY_DASHBOARD_HOST` | `ferry.<base-domain>` for local domains, otherwise off | serve the dashboard + API through the public proxy (`none` disables it). On a public domain, only enable it together with HTTPS. |
| `--acme-email` `FERRY_ACME_EMAIL` | – | enables Let's Encrypt |
| `--acme-staging` / `--acme-directory` | – | Let's Encrypt staging, or a custom ACME CA |
| `--github-webhook-secret` | – | enables `/hooks/github` |
| `--api-token` `FERRY_API_TOKEN` | generated | the server token: an API token that always works, for scripts on the server itself. Stored in `<data-dir>/api_token` (`0600`), never printed |
| `--name-prefix` `FERRY_NAME_PREFIX` | `ferry` | Docker resource prefix. Each Ferry server needs its own; a server refuses to start on a prefix owned by another data dir |
| `--take-over` | – | adopt the Docker resources of a prefix owned by another data dir (e.g. after moving it) |
| `--build-concurrency` `FERRY_BUILD_CONCURRENCY` | `2` | parallel builds |
| `--health-check-timeout` `FERRY_HEALTH_TIMEOUT` | `120` | seconds a deploy has to become healthy |
| `--default-port` `FERRY_DEFAULT_PORT` | `10000` | container port when nothing else specifies one (`PORT` is injected) |
| `--keep-images` `FERRY_KEEP_IMAGES` | `5` | built images kept per service for rollbacks |
| `--advertise-host` `FERRY_ADVERTISE_HOST` | `127.0.0.1` | host used in external datastore connection strings |
| `--default-memory-limit` `FERRY_DEFAULT_MEMORY_LIMIT` | `512M` | memory limit of each service, job and datastore container that sets none of its own (`512M`, `1G`, `1.5G`; `0` = unlimited) |
| `--default-cpu-limit` `FERRY_DEFAULT_CPU_LIMIT` | `1` | CPU limit likewise (`0.5`, `2`, `500m`; `0` = unlimited) |
| `--pids-limit` `FERRY_PIDS_LIMIT` | `1024` | max processes + threads per container, a fork-bomb guard (`0` = unlimited) |
| `--log-max-size` `FERRY_LOG_MAX_SIZE` | `10M` | rotate each container's Docker log at this size, with the `json-file` driver (`0` = keep the Docker daemon's log configuration). A daemon whose default log driver isn't `json-file` (journald, syslog, fluentd, `local`...) keeps it: Ferry only sets rotation over `json-file` |
| `--log-max-files` `FERRY_LOG_MAX_FILES` | `3` | log files kept per container when rotating (current one included) |
| `--min-free-disk` `FERRY_MIN_FREE_DISK` | `1G` | deploys and new datastores fail early when less is free on the data directory's (or the local Docker root's) filesystem (`0` = no check). Recreating an existing datastore's container is not blocked |
| `--oom-score-adj` `FERRY_OOM_SCORE_ADJ` | `-500` | Linux: ferryd's own OOM-killer score adjustment, -1000 to 1000, so the kernel kills containers before ferryd (`0` = leave unchanged). Lowering it needs root (or `CAP_SYS_RESOURCE`); otherwise use `OOMScoreAdjust=` in the systemd unit |

Two safety rules:
- Only one `ferryd` can use a data directory at a time (it holds a lock file).
- Each Docker name prefix belongs to one data directory. Without this, a second server would treat the first one's containers as orphans and remove them.

## Resource limits

Every container Ferry starts (service instances, one-off jobs, cron runs,
datastores) gets a memory limit, a CPU limit, a pids limit and log rotation.
A runaway app gets killed or throttled on its own, instead of taking down
the host, ferryd or your databases.

- **Server defaults:** 512 MiB and 1 CPU per container, set with
  `--default-memory-limit` / `--default-cpu-limit` (`0` = unlimited).
  `ferry info` shows them next to the Docker host's CPUs and memory.
- **Per service:** `ferry create` / `ferry update` / `ferry up` take
  `--memory 1G --cpu 0.5` (`default` goes back to the server default). The
  limits apply to each instance and to each job run. A change takes effect
  with the next deploy or restart; `ferry restart NAME` is enough (no
  rebuild).
- **Per datastore:** `ferry db create NAME --memory 1G`, and
  `ferry db update NAME --memory 2G --cpu 1` changes its container right
  away, without a restart (also for a failed datastore, e.g. one that ran out
  of memory: its next start uses the new limit). Lowering the memory below
  what the datastore uses can get it killed. Redis gets `maxmemory` at 3/4 of
  its memory limit, so it refuses writes when full instead of being killed.
- **Blueprints:** Render's `plan` becomes limits (`starter` → 512 MiB /
  0.5 CPU, `standard` → 2 GiB / 1 CPU, `pro` → 4 GiB / 2 CPUs, …; Postgres
  and Key Value plans set the memory). The Ferry keys `memoryLimit` and
  `cpuLimit` override it:

  ```yaml
  services:
    - type: web
      name: api
      plan: standard        # 2 GiB / 1 CPU
      cpuLimit: 500m        # but only half a core
  ```

  An apply restarts every live service whose limits changed (no rebuild).
  Re-applying an existing `render.yaml` after upgrading Ferry therefore
  restarts the services that have a `plan:` (it used to be ignored), with
  that plan's limits.

- **Dashboard:** a Resources section in each service's Settings and on each
  datastore's page.
- **Out of memory:** the kernel kills a container that goes over its memory
  limit, and Docker restarts it (a job run fails instead). A deploy whose new instance runs out of
  memory fails with `instance ab12cd ran out of memory (limit 512 MiB) —
  raise the service's memory limit`, `ferry status` marks the instance as
  OOM killed, and ferryd logs every OOM kill.
- **Host size:** a CPU limit above the Docker host's CPU count is capped at
  it, with a warning in the deploy log. On a host whose kernel has no CPU
  CFS quotas (some NAS / ARM kernels, rootless Docker without the `cpu`
  cgroup controller), where Docker refuses CPU limits, no CPU limit is set
  and ferryd and the deploy log say so.
- **Free disk:** deploys and new datastores fail early, with a clear message,
  when less than `--min-free-disk` (default `1G`) is free. Restarts,
  rollbacks and recreating an existing datastore's container still work.

Not covered: builds (BuildKit has no per-build memory limit), disk space
used by volumes and containers (no quotas), and network traffic (all apps
share one private network). See [DESIGN.md](DESIGN.md) §14–15 for the
details.

## How it works

```
 client ─► ferry-proxy (Host routing, TLS) ─► 127.0.0.1:<port> ─► app containers ─┐
 CLI / dashboard ─► ferry-api ─► ferry-engine ─► ferry-build (git + docker build)  │ ferry network
                         │            └──► ferry-docker ─► Docker daemon ◄────────┘ (private DNS)
                         └── SQLite (ferry-core::Store)
```

A deploy works like this:

1. Clone the repo or unpack the upload.
2. Detect the runtime and build the image.
3. Start new containers next to the old ones, then health-check them.
4. Switch the proxy to the new containers in one step.
5. Drain and stop the old ones.

A reconciler loop keeps Docker in line with the desired state. It replaces
crashed containers, enforces instance counts and refreshes routes.

See [DESIGN.md](DESIGN.md) for the full architecture, API reference, blueprint
format and internals.

## Web dashboard

The dashboard is a standalone React app in [`web/`](web/README.md), built with Vite,
TypeScript, Tailwind, shadcn/ui, Zustand and TanStack Query, and styled after Supabase
Studio. It talks to the API over HTTPS JSON (REST). Real-time updates use Server-Sent
Events: one change feed `/api/v1/events` that keeps the cache fresh, plus log streams.

```bash
cd web && npm ci
FERRY_API_URL=http://127.0.0.1:7878 npm run dev   # hot-reloading dev server (proxies /api to ferryd)
npm run build                                     # → web/dist, embedded by ferry-api's build script
```

## Development

```bash
cargo test --workspace                     # unit + API tests
FERRY_E2E=1 cargo test --workspace         # + Docker end-to-end tests
cargo clippy --workspace --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs the same checks on Linux for every pull request and every push to
`main`/`dev`: fmt, clippy and tests, the Docker end-to-end suites (`FERRY_E2E=1`), the web dashboard's
typecheck, lint, tests and build, and the documentation site (generated references in sync, typecheck, build
with link check).

Crates: `ferry-core` (models, store, contracts) · `ferry-docker` · `ferry-build` ·
`ferry-proxy` · `ferry-tls` · `ferry-engine` · `ferry-api` (REST + SSE + OpenAPI) ·
`ferry-cli` (`ferry`) · `ferryd`.

## License

MIT

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
| **Networking** | built-in reverse proxy (HTTP/1.1, HTTP/2, websockets), `name.your-domain`, custom domains, Let's Encrypt HTTPS, private network (`http://api:3000`) |
| **Data** | managed Postgres & Redis, persistent disks, env vars with references (`${{datastore.db.connectionString}}`), env groups |
| **Ops** | instance scaling, suspend/resume, restart, self-healing reconciler, live build & runtime logs, CPU/memory, one-off jobs |
| **Guardrails** | memory/CPU limits per service and datastore (server defaults 512 MiB / 1 CPU), pids limit and log rotation on every container, out-of-memory kills reported, free-disk check before deploys |
| **Interfaces** | web dashboard, `ferry` CLI, REST API, `ferry.yaml` / `render.yaml` blueprints |

## Quick start

Requirements: Docker (Docker Desktop on macOS works), Rust 1.85+ and Node.js 20+ to build.

```bash
(cd web && npm ci && npm run build)     # the web dashboard (embedded into ferryd at compile time)
cargo build --release
./target/release/ferryd                 # starts the server; prints the dashboard URL and API token
```

In another terminal:

```bash
./target/release/ferry login --server http://127.0.0.1:7878 --token <token printed by ferryd>

cd examples/node-hello
ferry up --follow                        # creates "node-hello", uploads, builds, deploys
curl http://node-hello.localhost:8080
```

Open the dashboard at **http://127.0.0.1:7878** (or http://ferry.localhost:8080). The interactive
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
ferry domains add api api.example.com
ferry update api --memory 2G && ferry restart api   # new limits apply with the next deploy or restart
ferry db update app-db --memory 1G                   # datastores: applied right away, no restart
```

## Production setup

1. A Linux VM with Docker. Point a wildcard DNS record `*.apps.example.com` (and
   your custom domains) at it.
2. Run `ferryd` as a service (systemd), for example:

   ```ini
   [Service]
   ExecStart=/usr/local/bin/ferryd \
     --data-dir /var/lib/ferry \
     --base-domain apps.example.com \
     --proxy-addr 0.0.0.0:80 --https-addr 0.0.0.0:443 \
     --acme-email you@example.com \
     --github-webhook-secret <random>
   Restart=always
   # When the host runs out of memory, let the kernel kill containers before ferryd.
   # (ferryd also sets -500 itself when it runs as root; see --oom-score-adj.)
   OOMScoreAdjust=-900
   # Keep some memory for ferryd under memory pressure (cgroup v2 hosts).
   MemoryMin=256M
   ```

   Containers (and builds, which run inside Docker) live in Docker's cgroups,
   not in this unit, so memory settings here only cover ferryd and the
   processes it starts (`git`, the `docker` CLI). Avoid a `MemoryMax=` on
   this unit: reaching it gets ferryd itself killed. Limit the apps with
   `--default-memory-limit` or per service instead (see
   [Resource limits](#resource-limits)).

3. Keep the API on localhost and reach it over SSH (`ssh -L 7878:127.0.0.1:7878 host`).
   You can also expose the dashboard through the proxy with
   `--dashboard-host ferry.apps.example.com`, but only with HTTPS enabled,
   because it serves the full admin API.
4. For GitHub auto-deploys, add a webhook to your repo:
   `https://ferry.apps.example.com/hooks/github`, content type JSON, with the
   same secret.

### Server configuration

Every flag also has an environment variable. `ferryd --help` shows the full list.

| Flag / env | Default | Meaning |
|---|---|---|
| `--data-dir` `FERRY_DATA_DIR` | `./ferry-data` | database, logs, build scratch, certs (created `0700`, since it holds secrets) |
| `--api-addr` `FERRY_API_ADDR` | `127.0.0.1:7878` | API + dashboard |
| `--proxy-addr` `FERRY_PROXY_ADDR` | `0.0.0.0:8080` | public HTTP |
| `--https-addr` `FERRY_HTTPS_ADDR` | – | public HTTPS (together with `--acme-email`) |
| `--base-domain` `FERRY_BASE_DOMAIN` | `localhost` | apps live at `<name>.<base-domain>` |
| `--public-port` `FERRY_PUBLIC_PORT` | proxy port | port shown in app URLs (e.g. behind port forwarding) |
| `--dashboard-host` `FERRY_DASHBOARD_HOST` | `ferry.<base-domain>` for local domains, otherwise off | serve the dashboard + API through the public proxy (`none` disables it). On a public domain, only enable it together with HTTPS. |
| `--acme-email` `FERRY_ACME_EMAIL` | – | enables Let's Encrypt |
| `--acme-staging` / `--acme-directory` | – | Let's Encrypt staging, or a custom ACME CA |
| `--github-webhook-secret` | – | enables `/hooks/github` |
| `--api-token` `FERRY_API_TOKEN` | generated | stored in `<data-dir>/api_token` (`0600`) |
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
| `--log-max-size` `FERRY_LOG_MAX_SIZE` | `10M` | rotate each container's Docker log at this size (`0` = keep the Docker daemon's log configuration) |
| `--log-max-files` `FERRY_LOG_MAX_FILES` | `3` | log files kept per container when rotating (current one included) |
| `--min-free-disk` `FERRY_MIN_FREE_DISK` | `1G` | deploys and new datastores fail early when less is free on the data directory's (or the local Docker root's) filesystem (`0` = no check) |
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
  `ferry db update NAME --memory 2G --cpu 1` changes the running container
  right away, without a restart.
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

- **Dashboard:** a Resources section in each service's Settings and on each
  datastore's page.
- **Out of memory:** the kernel kills a container that goes over its memory
  limit, and Docker restarts it (a job run fails instead). A deploy whose new instance runs out of
  memory fails with `instance ab12cd ran out of memory (limit 512 MiB) —
  raise the service's memory limit`, `ferry status` marks the instance as
  OOM killed, and ferryd logs every OOM kill.
- **Host size:** a CPU limit above the Docker host's CPU count is capped at
  it, with a warning in the deploy log.
- **Free disk:** deploys and new datastores fail early, with a clear message,
  when less than `--min-free-disk` (default `1G`) is free. Restarts and
  rollbacks still work.

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

Crates: `ferry-core` (models, store, contracts) · `ferry-docker` · `ferry-build` ·
`ferry-proxy` · `ferry-tls` · `ferry-engine` · `ferry-api` (REST + SSE + OpenAPI) ·
`ferry-cli` (`ferry`) · `ferryd`.

## License

MIT

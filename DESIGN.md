# Ferry — design & implementation contract

Ferry is a **self-hosted, single-node alternative to Render**, written in Rust.
You point it at a git repo (or a local directory, or a Docker image) and it
builds, deploys, routes, scales and heals your apps — web services, private
services, background workers, cron jobs and static sites — plus managed
Postgres/Redis, env groups, custom domains with automatic HTTPS, deploy
webhooks and `render.yaml`-compatible blueprints. Docker is the only runtime
dependency.

This document is the **contract** between crates. Public signatures in
`crates/*/src/lib.rs` are fixed; if you believe one must change, say so in
your report instead of changing it (except inside your own crate's private
code).

---

## 1. Render parity

| Render feature | Ferry |
|---|---|
| Web services, private services, background workers, cron jobs, static sites | ✅ `ServiceType` |
| Deploy from Git (auto-deploy on push) | ✅ GitHub webhook `/hooks/github`, any git URL incl. local paths |
| Connect GitHub / GitLab, pick a repository and a branch, deploy private repositories | ✅ git connections: the account authorizes the server on the provider's own pages — a GitHub App, a GitLab OAuth application — or with an access token (§18) |
| Deploy prebuilt Docker image | ✅ `image` / runtime `image` |
| Native runtimes (Node, Python, Go, Rust, Ruby, static) + Dockerfile | ✅ builder detection + generated Dockerfiles |
| Zero-downtime deploys + health checks | ✅ blue/green per deploy, `health_check_path` |
| Rollbacks, deploy history, build logs | ✅ |
| Manual deploy, deploy hooks (secret URL) | ✅ `/hooks/deploy/{id}?key=` |
| Env vars, secret files → env vars, environment groups | ✅ env vars + env groups (no secret files) |
| Private network (`service-name:port`) | ✅ user-defined Docker bridge + DNS aliases |
| Managed Postgres, Key Value (Redis) | ✅ containers + volumes, internal & external URLs |
| Persistent disks | ✅ named volume, recreate deploys, 1 instance |
| Custom domains + free TLS | ✅ ACME HTTP-01 (Let's Encrypt) |
| Manual scaling (instances) | ✅ round-robin in the proxy |
| Instance types (`plan`) | ✅ memory/CPU limits per service and datastore (server defaults 512 MiB / 1 CPU); blueprint `plan` → limits (§14) |
| Suspend / resume, restart | ✅ |
| One-off jobs | ✅ `ferry run` |
| Blueprints (`render.yaml`) | ✅ `ferry blueprint apply` (same schema, subset) |
| Logs (build + runtime, live tail), basic metrics | ✅ SSE streams, CPU/mem in status |
| Dashboard, CLI, REST API | ✅ |
| `ferry up` (deploy a local directory without git) | ➕ Ferry extra |

Out of scope for v1: multi-node, autoscaling, PR previews, private registries
auth, secret files, IP allow lists, teams/RBAC.

## 2. Architecture

```
                 ┌──────────────────────────── ferryd (one process) ─────────────────────────────┐
  browser /      │                                                                                 │
  curl     ────► │  ferry-proxy  :8080/:443 ── Host header ──► RouteTable ──► 127.0.0.1:<hostport> ─┼──► app containers
                 │      ▲   (ACME http-01, TLS SNI via ferry-tls)      ▲                             │     (ferry network)
                 │      │                                               │ swap after health check    │
  ferry CLI ───► │  ferry-api  :7878  (REST, SSE logs, webhooks, dashboard)                           │
  dashboard      │      │ Store (CRUD)          │ Arc<dyn Engine>                                     │
                 │      ▼                        ▼                                                     │
                 │  ferry-core::Store ◄──── ferry-engine ──► ferry-build (git + docker build)        │
                 │   (SQLite ferry.db)          │  deploy workers, reconciler, cron, datastores       │
                 │                              └──► ferry-docker (bollard) ──► Docker daemon          │
                 └─────────────────────────────────────────────────────────────────────────────────┘
```

### Crates (one owner each)

| crate | kind | depends on | responsibility |
|---|---|---|---|
| `ferry-core` | lib | — | models, DTOs, config, `Store` (SQLite), env resolution, naming, `Engine` trait, `TlsHooks` trait, validation, resource limits (`resources`: ranges, size / CPU parsing and formatting), cron schedules, git URL helpers, git connections (§18). **Frozen.** |
| `ferry-docker` | lib | core | typed Docker wrapper (containers, images, volumes, networks, logs, stats, exec) |
| `ferry-build` | lib | core | git fetch / archive extract, the branches of a remote (`git ls-remote`), runtime detection, Dockerfile generation, `docker build` |
| `ferry-scm` | lib | core | git providers (§18): GitHub / GitLab REST clients, authorizing an account in the browser (GitHub App manifest + installation, GitLab OAuth), the tokens of a connection (minted or renewed on demand), which connection reads a repository |
| `ferry-proxy` | lib | core | `RouteTable`, HTTP/HTTPS reverse proxy, websockets, error pages |
| `ferry-tls` | lib | core | ACME certificates, SNI resolver, `TlsHooks` impl |
| `ferry-engine` | lib | core, docker, build, proxy, scm | `FerryEngine: Engine` — deploys, reconciler, cron, jobs, datastores, logs |
| `ferry-api` | lib | core, build, scm | axum router: REST, SSE (logs + `/api/v1/events`), webhooks, blueprints, OpenAPI + Swagger UI, git connections (§18; `build` only for the branches of a remote), serves the embedded web client (`web/dist`) |
| `ferry-cli` | bin `ferry` | core | CLI client |
| `ferryd` | bin | all | wiring (already written) |

Leaf crates must not depend on each other beyond this table.

## 3. Networking model (important on macOS)

* One user-defined bridge network per server: `Naming::network()` (= `name_prefix`, default `ferry`).
* Every service container joins it with DNS alias **`<service-name>`**; every
  datastore with alias **`<datastore-name>`**. Services talk to each other at
  `http://<name>:<port>` and to datastores via `internal_url()`.
* Containers that listen (web, private, static) publish their port on
  **`127.0.0.1:<ephemeral>`** (`PortPublish { host_ip: "127.0.0.1", host_port: None }`).
  The proxy and health checks connect to `127.0.0.1:<host_port>`. **Never use
  container IPs** — they are unreachable from the host on Docker Desktop.
  Ephemeral host ports can change when Docker restarts a container, so the
  engine refreshes routes from `inspect`/`list` data in every reconcile pass.
* Datastores publish on `config.datastore_bind_ip:<fixed host_port>` (allocated
  once with `ferry_docker::free_host_port()` and stored).
* Public hostnames: `<name>.<base_domain>` (default `*.localhost`, which
  browsers resolve to 127.0.0.1) plus custom domains. Dashboard:
  `ferry.<base_domain>` routed to the API address (service id `__dashboard`).

## 4. Docker resource naming & labels

Always use `ferry_core::Naming` (from `config.naming()`):

* images `"{prefix}/{service}:{deploy_id}"`; service containers
  `"{prefix}-{service}-{deploy_short}-{rand6}"`; jobs `"{prefix}-job-{service}-{job_short}"`;
  datastores `"{prefix}-ds-{name}"` with volume `"{prefix}-ds-{name}-data"`;
  service disks `"{prefix}-svc-{service_id}-disk"`.
* Labels: `ferry.managed=true`, `ferry.instance={prefix}`, `ferry.role=service|job|datastore`,
  `ferry.service`, `ferry.deploy`, `ferry.job`, `ferry.datastore`. The engine
  only ever lists/touches containers whose `ferry.instance` equals its prefix.

## 5. Deploy pipeline (ferry-engine)

States: `queued → building → deploying → live` (previous live → `deactivated`);
failures `build_failed` / `deploy_failed`; `canceled`.

1. **Enqueue** (`Engine::deploy`): resolve the source — explicit, else from the
   service: `image` → `DeploySource::Image`; `repo_url` → `Git{branch, commit}`;
   neither → the most recent `Archive` deploy of the service (Invalid error if
   none: "upload code with `ferry up`"). Suspended service → Conflict. Insert the
   deploy (`queued`), cancel older *queued* deploys of the same service
   (`canceled`, error "superseded by dep-…"), wake the service's worker, return.
2. **One worker per service** processes its queue in order; a global semaphore
   of `config.build_concurrency` bounds concurrent builds.
3. **Build** (`building`): Git, Archive and Image deploys first run the
   free-disk check (§14): too little free space fails the deploy
   (`build_failed`) with a message naming the path, the free space and the
   threshold. Reuse deploys (restart, rollback) skip it: they neither build
   nor pull, and must keep working on a host that is short on disk.
   Git/Archive → `Builder::build` with image tag
   `naming.image_tag(name, deploy_id)`, build args = the service's resolved env,
   labels `naming.service_labels`. A Git deploy whose repository is served
   by one of the server's git connections (§18) clones with a token of that
   connection (`BuildSource::Git.credentials`), after `==> Cloning with the
   GitHub account 'octocat'`. Image → `docker.ensure_image` (pull; always
   pull when the tag is `latest`/untagged). Reuse → verify the image exists.
   Record `image`, `commit_sha`, `commit_message` on the deploy (the commit is
   recorded as soon as it is checked out — `BuildEvent::CheckedOut` — so failed
   builds show it too). Pulled images are **pinned**: tagged locally as
   `naming.image_tag(name, deploy_id)`, which becomes `deploy.image` (the
   user's reference stays in `source.image`), so rollbacks are exact even when
   `:latest` moves. Build-time env reaches builds as **BuildKit secrets**
   (`--secret id=KEY,env=KEY`; generated Dockerfiles mount them per `RUN`),
   never as `ARG`s, so values don't end up in the image history.
   A generated Dockerfile's build log says which command the image starts
   and where it comes from (`GeneratedDockerfile.start`): `==> Start
   command: npm start (the "start" script of package.json)` — the service's
   start command, the Procfile, or the runtime's fallback, e.g. `node
   ./dist/index.js (package.json has no "start" script: its "main" file is
   run)`. A command nobody chose is the first thing to look at when an
   instance doesn't stay up.
   Cron jobs stop here: log their limits (`==> Limits: … per run`), mark
   `live`, set `live_deploy_id`, deactivate previous.
4. **Start** (`deploying`): port = `env::choose_port(service.port, user_env,
   build.port_hint, docker.image_exposed_ports(image), config.default_port)`
   for listening services (store it in `deploy.port`); env = `env::container_env(
   env::injected(..), env::resolve_all(store.effective_env, RefContext))`;
   `cmd = ["/bin/sh","-c", start_command]` when a start command is set and the
   runtime is `docker`/`image` (generated Dockerfiles already bake it in);
   restart policy `UnlessStopped`; network + alias; disk volume if configured;
   the resources of §14 (the service's memory / CPU limits captured in the
   launch spec, else the server defaults; pids limit; log rotation), logged
   as `==> Limits: 512 MiB memory, 1 CPU per instance` (plus a
   `==> Warning:` line when the CPU limit was capped at the host's CPUs or
   the memory limit exceeds the host's memory).
   Start `desired_instances()` new containers.
   * **Services with a disk** use *recreate*: stop + remove old containers first.
   * Everything else is *blue/green*: old containers keep serving.
5. **Health check** each new instance, polling every 1s up to
   `config.health_check_timeout_secs`:
   * container exited/dead (or already restarted by Docker) → fail
     immediately, copy its last 50 log lines into the deploy log. The error
     says why: when Docker's `OOMKilled` flag is set,
     `instance ab12cd ran out of memory (limit 512 MiB) — raise the service's memory limit`
     (without a memory limit: killed by the kernel's OOM killer because the
     host ran out of memory); otherwise the exit code, e.g.
     `instance ab12cd crashed (exit code 137: killed by SIGKILL)` — 137 alone
     is no proof of an OOM kill. Docker restarts a crashed instance within
     ~100 ms and then clears `OOMKilled` and the exit code, so for one seen
     running again the engine polls ~3 s for it to be restarting or exited
     again, then reads the container's Docker events since it started
     (`die` with its `exitCode`, `oom`; a bounded query with `until` = now):
     an app that runs a while before each OOM kill is still reported as out
     of memory;
   * exit code 0 is explained (`health::clean_exit`): nothing crashed and
     often nothing was printed, so the error names what ran (read from
     Docker before the instance is removed: its command, a shell wrapper
     shown as its script) and why ending is a failure —
     `instance ab12cd exited with code 0: its command `node ./dist/index.js`
     ended without an error, but a web service must keep running and listen
     on port 10000` — and the failure carries a hint that depends on where
     the command comes from: the service's start command, the image's own
     command (`docker` / `image` runtimes), or picked from the project for a
     service without a start command (then: set the start command of an
     app; create a project that only builds files as a static site).
     Workers are told that what runs and ends belongs in a cron job or a
     one-off job;
   * web/private/static with `health_check_path`: `GET http://127.0.0.1:<host_port><path>`
     with `Host: <default host>` → success on status < 400;
   * without path: TCP connect to `127.0.0.1:<host_port>` succeeds;
   * workers: still running 5s after start.
   Stream new containers' output into the deploy log while waiting.
6. **Swap**: web/static → `routes.set_service_routes(id, config.service_hosts(svc),
   new upstreams)`; immediately mark the deploy `live` (`live_deploy_id`,
   previous live → `deactivated`) — so a crash while draining can't cause an
   outage — then stop (10s grace) and remove old instances. Log
   `==> Your service is live 🎉` and the URL. A cancel after the swap is a
   Conflict ("too late to cancel").
   The deploy's full container spec (image, resolved env, command, port,
   disk, memory / CPU limits) is **snapshotted** when it goes live. The
   reconciler, scale, resume and one-off jobs always start instances from the
   live deploy's snapshot —
   never from current settings/env — so settings and env changes take effect
   only through a deploy or restart (like Render), and a failed env-change
   deploy can't break self-healing.
7. **Failure**: remove new containers, keep old ones serving, status
   `build_failed`/`deploy_failed` with a concise `error`. When the engine
   can tell what to do (a clone the remote refused, §18; an instance whose
   command ended), a `==> Hint: …` line follows the failure line. The
   server log gets one line per failed deploy, at `WARN`, with the same
   content: `deploy failed: <error>` and the fields `service` (its name),
   `deploy`, `status` and `hint` (`-` when there is none); canceled deploys
   are logged at `INFO`. The app's own output stays in the deploy log.
8. **Cleanup**: keep the newest `config.keep_images` images of this service
   (never the live one; only images whose repo is `naming.image_repo(name)` —
   never delete pulled public images); delete scratch dirs.
9. **Cancel**: queued → canceled at once; building → cancel token (builder
   kills `docker build`); deploying → remove new containers. Terminal → Conflict.

Log conventions (deploy log): system lines start with `==> ` (e.g.
`==> Cloning from https://github.com/a/b (branch main)`,
`==> Using Dockerfile at ./Dockerfile`,
`==> Start command: npm start (the "start" script of package.json)`,
`==> Build successful 🎉`,
`==> Limits: 512 MiB memory, 1 CPU per instance`, `==> Starting 2 instance(s)`,
`==> Health check passed`, `==> Your service is live 🎉`).

## 6. Reconciler (ferry-engine)

Runs at boot (synchronously, before the proxy serves) and every 10s, and on
demand after scale/suspend/resume:

* per service (skipping services with an in-flight deploy in the *deploying*
  phase): containers labelled with the service → keep running containers of
  the live deploy up to `desired_instances()`; remove exited/dead ones and
  extras; start missing instances (same spec as the deploy); remove containers
  of non-live deploys.
* routes: suspended → `set_service_suspended`; otherwise running live
  containers that accept TCP connections → `set_service_routes` (empty list →
  proxy 503). Remove routes of deleted services (keep `__dashboard`).
* datastores: ensure container exists & running (volumes keep data). A
  container created here gets the row's limits; limits are not re-applied
  to existing containers (only `PATCH /api/v1/datastores/{id}` does that).
* orphans: containers with our `ferry.instance` label whose service/datastore
  no longer exists → remove. Job containers of finished/unknown jobs → remove.
* boot only: deploys still `queued/building/deploying` → `build_failed` /
  `deploy_failed` with error "interrupted by server restart"; jobs still
  `pending/running` → `failed`.

Next to the loop, an **OOM watcher** (spawned at engine start) follows
Docker's `oom` events for this server's containers (`type=container`,
`event=oom`, `label=ferry.instance=<prefix>`) and logs each kill as a
server-log warning naming the instance / job / datastore, its owner and the
limit it hit, e.g. `instance a1b2c3 of service 'web' ran out of memory (limit
512 MiB) — raise the service's memory limit`. After a reconnect it replays
the events it missed (`since=<secs>.<nanos>`). It uses events rather than the
reconcile pass because Docker clears a container's `OOMKilled` flag as soon
as the restart policy runs it again (on Docker Desktop, immediately), so
polling would miss most kills. Nothing is written to the service's runtime
log (runtime logs are Docker's container logs).

## 7. Jobs & cron

* Scheduler ticks every ~15s; for each non-suspended cron job with a live
  deploy, `Schedule::fires_between(last_tick, now)` → `run_job(Schedule)`.
  Missed runs while the server was down are skipped. If a previous run of the
  same service is still running, skip (log at info).
* A job run: container `naming.job_container`, labels `naming.job_labels`,
  image = live deploy image, cmd = `sh -c <command>` (override, else service
  start command, else image default), env like the service (no `PORT`),
  network joined (reaches datastores / private services), no published port,
  restart `No`, the live deploy's resources (§14: the service's memory / CPU
  limits from the live launch spec — each run gets its own, on top of the
  instances' — plus the pids limit and log rotation; capping / memory
  warnings go to the job log). Output streamed into `logs/jobs/<id>.log`;
  status `running` → `succeeded` (exit 0) / `failed` (`error`: `exited with
  code N`, or `the job ran out of memory (limit 64 MiB) — raise the service's
  memory limit` when Docker's `OOMKilled` flag is set); container removed
  after.
* `run_job` on non-cron services requires `command`; needs a live deploy.
  One-off jobs mount the service's disk.

## 8. Datastores

* Postgres: image `postgres:{version}-alpine`, env `POSTGRES_USER`,
  `POSTGRES_PASSWORD`, `POSTGRES_DB`; volume at `/var/lib/postgresql/data`;
  ready when `pg_isready -U <user> -d <db>` (exec) exits 0.
* Redis: image `redis:{version}-alpine`, cmd `redis-server --requirepass <pw>
  --appendonly yes`; volume at `/data`; ready when `redis-cli -a <pw> ping` → `PONG`.
  Its entrypoint is a small `sh -c` script (`REDIS_START` in
  `ferry-engine/src/datastores.rs`) that reads the container's memory limit
  from its cgroup (`memory.max`, else cgroup v1's `memory.limit_in_bytes`) on
  every start, appends `--maxmemory <3/4 of it>` (none when unlimited), then
  runs the image's own `docker-entrypoint.sh` (which drops root). Over
  `maxmemory` Redis refuses writes (`OOM command not allowed`, Redis's
  default `noeviction` policy) instead of the kernel killing it — and killing
  it again while it replays an append-only file that no longer fits; the
  other quarter is headroom for the AOF rewrite's fork, client buffers and
  fragmentation.
* Publish internal port on `datastore_bind_ip:host_port`; alias = name;
  restart `UnlessStopped`. Status `creating → available | failed` (error set).
* Resources (§14): the container is created with the row's memory / CPU
  limits (else the server defaults), the pids limit and log rotation;
  capping / memory warnings go to the server log (datastores have no log of
  their own). The free-disk check runs before a **new** datastore's volume
  and container are created (its volume doesn't exist yet; checked before
  the volume is created, so a retry still counts as new), and before an
  image pull; recreating the container of an existing datastore whose image is
  present (it is dead, was removed, can't be started...) needs no space and
  is not checked, so the reconciler still heals it on a full disk. A
  container OOM-killed while starting fails provisioning at once with
  `the postgres container ran out of memory (limit 256 MiB) — raise the datastore's memory limit`.
  The failed container stays (`unless-stopped`: Docker keeps restarting it)
  and the reconciler's retry keeps it.
* Provisioning that keeps an existing container (running, restarting, or a
  stopped one it starts) first applies the row's limits to it when they
  differ from the container's configured `Memory` / `NanoCpus` (a quota of
  every host CPU counts as none): limits raised while the datastore was
  `failed` — the advice of the OOM message above — reach the container
  (a restarting one gets them for its next start). A failed update there is
  a server-log warning, not a provisioning failure.
* Limits change **in place**: `Engine::update_datastore_limits` applies the
  row's limits to the datastore's container with `docker update` (no
  restart; lowering memory below what it uses can get it killed), whatever
  the datastore's status; Redis also gets `CONFIG SET maxmemory` (3/4 of the
  new limit; best effort, a Redis that isn't running gets it from its start
  script). No container yet (or not ours, or dead / being removed: it is
  recreated with the row's limits) → Ok; container removed meanwhile → Ok;
  datastore being deleted → Conflict; any other Docker failure →
  `Error::Docker("changing the limits of the <kind> container failed: …")`,
  password redacted. It doesn't take the datastore's lock (a PATCH must not
  wait behind a provisioning that can take a minute): applying limits —
  here, and in provisioning right after it creates or keeps a container —
  reads the row and runs `docker update` under a short per-datastore
  *limit* lock (`datastore_limit_locks`), so of two concurrent applies the
  later one always applies the newest row.
* Docker can't *remove* a memory or CPU limit with `docker update` (`Memory`
  / `NanoCpus` 0 mean "unchanged", negative values are rejected), so
  `Docker::update_limits` spells "no limit" (e.g. a server default of 0) as
  a limit nothing reaches: memory and swap 4e18 bytes (`NO_MEMORY_LIMIT` in
  `ferry-docker`; inspect reports any limit from 1 EiB up as none) and a CPU
  quota of all the host's CPUs (read from `docker info`; inspect reports it
  as is). `PidsLimit: -1` does remove the pids limit.
* Delete: remove container + volume, then the row.
* A new datastore never adopts a pre-existing volume of the same name (error
  instead). Readiness probes pass secrets via the exec environment
  (`REDISCLI_AUTH`), never argv; passwords never appear in logs or errors.

## 9. Logs

* Deploy logs → `logs/deploys/<deploy_id>.log`; job logs → `logs/jobs/<job_id>.log`;
  one JSON `LogLine` per line.
* `deploy_logs(id, follow)` / `job_logs`: replay the stored lines; with
  `follow` and the deploy/job still active, continue with new lines (no gaps,
  no duplicates) and end when it reaches a terminal state.
* Runtime logs: `docker.logs` of each current container, merged; `instance`
  = last 6 chars of the container name.
* API streams as **SSE**: `event: log` + `data: <LogLine JSON>`; finite streams
  end with `event: end` + an empty `data:` line (so EventSource dispatches it).
  Keep-alive comments every 15s. All streams end when the server shuts down.

## 10. HTTP API (`ferry-api`)

Auth: `Authorization: Bearer <token>` on `/api/*` (except the OpenAPI
document and Swagger UI below); GET requests may use `?access_token=<token>`
instead (EventSource can't set headers). Constant-time compare. Errors:
`ApiErrorBody` with `Error::status()`. `{id}` = id or name. JSON bodies; 404
JSON for unknown `/api` routes (behind the token, like every `/api` path).

**OpenAPI.** Every handler carries a `#[utoipa::path]` (method, path, tag,
params, bodies, responses with `ApiErrorBody` errors); `openapi::ApiDoc`
(`#[derive(OpenApi)]`, OpenAPI **3.1**) gathers them with every DTO schema,
a `bearer` HTTP security scheme (required by every `/api/v1` operation, none
for `/healthz`, the webhooks and the document) and one tag per resource. It is
served at `GET /api/openapi.json`, and Swagger UI (`utoipa-swagger-ui`, assets
vendored into the binary: works offline, no validator call) at `/api/docs`;
both without auth, and outside the token-checking API router so neither
fallback can shadow them. `tests/openapi.rs` keeps one authoritative list of
routes and checks that the document, the router and `lib.rs` agree.

| Method & path | Body → Response |
|---|---|
| `GET /healthz` | `ok` (no auth) |
| `GET /` (and any other non-API path) | the web client (§13; SPA fallback, no auth) |
| `GET /api/openapi.json` | the OpenAPI 3.1 document (no auth) |
| `GET /api/docs` | Swagger UI (no auth; redirects to `/api/docs/`, its **Authorize** takes the API token) |
| `GET /api/v1/info` | `ServerInfo` (including the default limits `default_memory_limit_mb` / `default_cpu_limit`, 0 = unlimited, and the Docker host's `docker_cpus` / `docker_memory_bytes`, `null` when unknown) |
| `GET /api/v1/events` | SSE change feed: `event: ready` (`data: {}`) once the feed watches the store (refetch after it), then `event: change` with `ChangeEvent` `{kind, id, service_id, action}` — `kind` ∈ `service`/`deploy`/`datastore`/`env_group`/`job`/`git_connection`, `action` ∈ `created`/`updated`/`deleted`, `service_id` set for services (own id), deploys and jobs; a lagging subscriber gets `{kind:"all", id:"*", service_id:null, action:"resync"}` (refetch everything). The store is polled every second while someone listens and nudged after every API/webhook write |
| `GET /api/v1/services` | `[ServiceView]` |
| `POST /api/v1/services` | `CreateService` → 201 `ServiceView` (queues a `create` deploy when it has a repo/image, unless `deploy:false`) |
| `GET /api/v1/services/{id}` | `ServiceView` |
| `PATCH /api/v1/services/{id}` | `UpdateService` → `ServiceView` (instances → `engine.scale`, suspended → `engine.suspend/resume`, custom_domains → `engine.refresh_routes`; the rest is stored — `memory_limit_mb` / `cpu_limit` too, `0` = back to the server default: they apply with the next deploy or restart, the PATCH doesn't redeploy) |
| `DELETE /api/v1/services/{id}?force=` | 204 (`engine.delete_service`); 409 listing the referencing services when other services reference it via `${{service.…}}`, unless `force=true` |
| `GET /api/v1/services/{id}/status` | `RuntimeStatus` (per instance: CPU / memory usage, the container's configured `memory_limit_bytes` / `cpu_limit` — `null` = unlimited —, `oom_killed`, `exit_code`) |
| `GET /api/v1/services/{id}/logs?follow=&tail=` | SSE |
| `POST /api/v1/services/{id}/restart` | 202 `Deploy` |
| `POST /api/v1/services/{id}/suspend` · `/resume` | `ServiceView` |
| `POST /api/v1/services/{id}/scale` | `ScaleRequest` → `ServiceView` |
| `POST /api/v1/services/{id}/rollback` | `RollbackRequest` → 202 `Deploy` (only to deploys that went live) |
| `POST /api/v1/services/{id}/deploy-hook/rotate` | → `ServiceView` with a new `deploy_hook_path` (old URL stops working) |
| `GET /api/v1/services/{id}/deploys?limit=20` | `[Deploy]` newest first |
| `POST /api/v1/services/{id}/deploys` | `TriggerDeploy` → 202 `Deploy` |
| `POST /api/v1/services/{id}/deploys/upload?clear_cache=` | raw `.tar.gz` body (≤ 512 MiB) saved to `uploads/<new id>.tar.gz` → 202 `Deploy` (trigger `upload`, source `Archive`) |
| `GET /api/v1/deploys/{deploy_id}` | `Deploy` |
| `POST /api/v1/deploys/{deploy_id}/cancel` | `Deploy` |
| `GET /api/v1/deploys/{deploy_id}/logs?follow=` | SSE |
| `GET /api/v1/services/{id}/env` | `[EnvVar]` |
| `PUT /api/v1/services/{id}/env?restart=` | `ReplaceEnv` → `[EnvVar]` |
| `PATCH /api/v1/services/{id}/env?restart=` | `PatchEnv` → `[EnvVar]` (`restart=true` → `engine.restart(EnvChange)` if live) |
| `POST /api/v1/services/{id}/env-groups` | `LinkEnvGroup` → `ServiceView` |
| `DELETE /api/v1/services/{id}/env-groups/{group}` | `ServiceView` |
| `GET /api/v1/services/{id}/domains` | `[String]` custom domains |
| `POST /api/v1/services/{id}/domains` | `DomainRequest` → `[String]` (validated, unique across services, not a default host) |
| `DELETE /api/v1/services/{id}/domains/{domain}` | `[String]` |
| `GET /api/v1/services/{id}/jobs?limit=20` | `[JobRun]` |
| `POST /api/v1/services/{id}/jobs` | `RunJobRequest` → 202 `JobRun` |
| `GET /api/v1/jobs/{job_id}` | `JobRun` |
| `POST /api/v1/jobs/{job_id}/cancel` | `JobRun` (`canceled`); 409 when it already finished |
| `GET /api/v1/jobs/{job_id}/logs?follow=` | SSE |
| `GET /api/v1/datastores` | `[DatastoreView]` |
| `POST /api/v1/datastores` | `CreateDatastore` → 201 `DatastoreView` (row `creating`, with its optional `memory_limit_mb` / `cpu_limit`, then `engine.provision_datastore`) |
| `GET /api/v1/datastores/{id}` · `DELETE ?force=` | `DatastoreView` · 204 (409 while referenced, unless `force=true`) |
| `PATCH /api/v1/datastores/{id}` | `UpdateDatastore` `{memory_limit_mb?, cpu_limit?}` → `DatastoreView`. Stores the limits (`0` = back to the server default; an omitted field keeps its value) under the datastore's row lock, then `engine.update_datastore_limits` applies them to its container in place, without a restart, whatever the status (also when nothing changed, so re-sending retries a failed apply): a `failed` datastore's container may still be there, restarting with the old limits, and the retry keeps it (§8); one without a container yet gets them when it is created. A failed apply after the save → the engine's error prefixed `limits of 'NAME' saved, but applying them to its container failed: ` (502 for Docker errors, 409 while the datastore is being deleted) |
| `GET /api/v1/env-groups` · `POST` | `[EnvGroupView]` · `CreateEnvGroup` → 201 `EnvGroupView` |
| `GET /api/v1/env-groups/{id}` · `DELETE ?force=&restart=` | `EnvGroupView` · 204 (409 while linked, unless `force=true`; `restart=true` restarts the linked live services) |
| `PUT` / `PATCH /api/v1/env-groups/{id}/env?restart=` | `ReplaceEnv` / `PatchEnv` → `EnvGroupView` (restart linked live services) |
| `GET /api/v1/git/connections` | `[GitConnectionView]` (§18; no secret is ever returned, of a personal token only `token_hint`). `status` is `pending` for an authorization that was started and not finished |
| `POST /api/v1/git/authorize` | `AuthorizeGit` `{provider?, base_url?, connection_id?, redirect_uri, organization?, client_id?, client_secret?}` → `GitAuthorization` `{status, url, method, fields, connection}`: where to send the browser (`status: redirect`; `method: post` = submit a form with `fields`), or the connection when nothing is left to do (`status: connected`). GitHub: the page that registers the server's GitHub App from a manifest. GitLab: the page where the account authorizes the server's OAuth application — 400 `git_application_required` until its `client_id` / `client_secret` were given once. `connection_id` resumes a pending connection or authorizes one again. 400 `git_authorization_rejected`, 502 `git_provider_unavailable` |
| `POST /api/v1/git/callback` | `GitCallback` `{state, code?, installation_id?, setup_action?, error?, error_description?}` — the query parameters the provider sent the browser back with → `GitAuthorization`: the next page (GitHub: install the app that was just registered) or the connection. A `state` works once, for an hour; without one, an `installation_id` re-reads that installation. 400 for an unknown `state`, 400 `git_authorization_rejected` when the provider or the user refused, 502 `git_provider_unavailable` |
| `POST /api/v1/git/connections` | `ConnectGit` `{provider, token, base_url?}` → 201 `GitConnectionView`: the way without a browser — asks the provider whose token it is, then stores it. 200 when that account was already connected (the token replaces what it had). 400 `git_authorization_rejected` when the provider rejects the token, 502 `git_provider_unavailable` when it can't be reached |
| `GET /api/v1/git/connections/{id}` · `DELETE ?force=` | `GitConnectionView` · 204 (409 while it clones the repository of services, unless `force=true`: they are cloned without credentials from then on). `{id}` is the id only |
| `GET /api/v1/git/connections/{id}/repositories` | `GitRepositoryList` `{repositories, truncated}`: what the connection can read, most recently updated first, asked from the provider on every call (at most 1000). 409 `git_authorization_rejected` when the connection is pending or the provider no longer accepts its authorization, 502 `git_provider_unavailable` |
| `GET /api/v1/git/branches?repo_url=` | `GitBranches` `{default_branch, branches, connection_id}`: the branches of any repository a service can deploy from, read from its remote (`git ls-remote`, 20 s) the way a deploy would clone it; the default branch first, then by name. 400 for a malformed URL, 502 `git_remote_unreachable` |
| `POST /api/v1/blueprints/apply` | `ApplyBlueprint` JSON, or raw YAML (`Content-Type: application/yaml` / `text/yaml`, `?dry_run=`) → `BlueprintResult` |
| `GET\|POST /hooks/deploy/{service_id}?key=` | 202 minimal deploy summary (trigger `deploy_hook`, credentials redacted); service **id** only; unknown id or wrong key → the same 401 (no enumeration) |
| `POST /hooks/github` | GitHub webhook. 404 unless `github_webhook_secret` set; verify `X-Hub-Signature-256` (HMAC-SHA256, constant-time) → 401; `ping` → 200; `push` → deploy (trigger `webhook`, commit = `after`) every service with `auto_deploy`, matching `git::normalize_repo_url` of `repository.clone_url`/`ssh_url`/`html_url`, and `refs/heads/<branch>`; deleted-branch pushes ignored → 200 `{"deploys": [...]}` |

Name rules: `validate::resource_name` for services/datastores (names shared
between both; id-shaped names are rejected), `validate::env_group_name`,
`validate::env_vars` (32 KiB per value, 256 KiB per owner, also checked on
the merged service + linked groups). Resource limits (`memory_limit_mb` /
`cpu_limit` on `CreateService`, `UpdateService`, `CreateDatastore`,
`UpdateDatastore`) are checked by `resources::validate` — 16 MiB to 1 TiB,
0.01 to 512 CPUs — with the CPU value rounded to 0.01; on create, an omitted
limit or `0` means the server default, as on PATCH. Request bodies reject
unknown fields.
Git inputs are validated up front (`validate::repo_url` — absolute local
paths only —, `branch`, `commit`). Switching a service between git and image
requires clearing the other source in the same PATCH. A service names no
git connection: its repository is cloned with the connection of the server
that serves `repo_url` (§18). Cron jobs always have
exactly 1 instance. Writes to one service are serialized, and custom-domain
claims are globally serialized. GitHub webhooks also accept form-encoded
deliveries and ignore replayed payloads.

## 11. Blueprints (`ferry.yaml` / `render.yaml`)

Render's schema (camelCase), a pragmatic subset:

```yaml
envVarGroups:            # [{name, envVars: [{key, value} | {key, generateValue: true}]}]
databases:               # Postgres: {name, databaseName?, user?, postgresMajorVersion?, plan?, memoryLimit?, cpuLimit?}
services:
  - type: web|pserv|worker|cron|static|redis|keyvalue   # web + runtime: static → static site
    name: api
    plan: standard                         # → memory / CPU limits (see below)
    memoryLimit: 1G                        # Ferry extension: overrides the plan's memory (1G, 512M, or MiB)
    cpuLimit: 500m                         # Ferry extension: overrides the plan's CPU (0.5, 2, or millicores)
    runtime: docker|image|node|python|go|rust|ruby|static   # legacy key `env` accepted
    repo: https://github.com/org/repo      # or a local path
    branch: main
    rootDir: ./api
    dockerfilePath: ./Dockerfile
    dockerCommand: ./start.sh              # → start_command
    buildCommand: npm ci && npm run build
    startCommand: npm start
    staticPublishPath: ./dist              # → publish_dir
    image: { url: nginx:alpine }           # or a plain string
    port: 3000                             # Ferry extension
    healthCheckPath: /healthz
    numInstances: 2
    autoDeploy: true
    schedule: "*/5 * * * *"
    disk: { name: data, mountPath: /var/data, sizeGB: 1 }
    domains: [app.example.com]
    envVars:
      - { key: NODE_ENV, value: production }
      - { key: SECRET, generateValue: true }       # only generated if not already set
      - { key: DB_URL, fromDatabase: { name: app-db, property: connectionString } }
      - { key: REDIS_URL, fromService: { type: redis, name: cache, property: connectionString } }
      - { key: API_HOST, fromService: { type: pserv, name: api, property: hostport } }
      - { key: TOKEN, fromService: { type: web, name: api, envVarKey: TOKEN } }   # copied at apply time
      - { fromGroup: shared-settings }
      - { key: STRIPE_KEY, sync: false }            # left for the user to set → warning
```

* `fromDatabase` / `fromService` compile to `${{datastore.NAME.PROP}}` /
  `${{service.NAME.PROP}}` references (resolved at container start).
  Render properties map: `connectionString`, `host`, `port`, `user`,
  `password`, `database`, `hostport`.
* `redis` / `keyvalue` services create Redis datastores.
* Unknown / unsupported keys (`region`, `scaling`, `previews`, …) are
  ignored with a warning, never an error.
* **`plan` → resource limits** (`ferry_api::blueprint::plans`; names are
  case-insensitive and multi-word plans accept any separator: `pro plus`,
  `Pro_Plus`, `pro-plus`, `proplus`):

  | entry | plan → memory / CPU |
  |---|---|
  | web, pserv, worker, cron | `free`, `starter` → 512 MiB / 0.5 CPU · `standard` → 2 GiB / 1 · `pro` → 4 GiB / 2 · `pro plus` → 8 GiB / 4 · `pro max` → 16 GiB / 4 · `pro ultra` → 32 GiB / 8 |
  | Postgres (`databases`) | `<tier>-<size>` with tier `basic`, `pro` or `accelerated` (`basic-256mb`, `basic-1gb`, `pro-4gb`, `accelerated-16gb`) → that memory (a unit is required, and it must be a valid limit); legacy `free`, `starter` → 256 MiB · `standard` → 1 GiB · `pro` → 4 GiB · `pro plus` → 8 GiB. CPU: not set |
  | Key Value (`redis` / `keyvalue`) | `free`, `starter` → 256 MiB · `standard` → 1 GiB · `pro` → 5 GiB · `pro plus` → 10 GiB. CPU: not set |
  | static sites | `plan` ignored silently (Render has no plans for them); `memoryLimit` / `cpuLimit` still apply, since Ferry runs them in containers |

  Render's `free` plan is 0.1 CPU; Ferry gives `free` and `starter` 0.5
  CPU, because on your own server a tenth of a core only makes a service
  slow (builds of interpreted apps, startup, health checks) and frees
  nothing the host would otherwise use. An unknown plan is a warning
  (`service 'x': ignoring unknown plan 'y' (known plans: …); set
  memoryLimit / cpuLimit instead`) and sets nothing.
* **`memoryLimit` / `cpuLimit`** (Ferry extensions, parsed with
  `resources::parse_memory_mb` / `parse_cpus`) override the plan key by key:
  `plan: pro` + `memoryLimit: 1G` = 1 GiB / 2 CPUs. `0` = explicitly the
  server default. A malformed value is an error naming the entry
  (`service 'a': 'memoryLimit': invalid memory size 'lots' …`), and so is a
  non-zero `cpuLimit` that rounds to 0 (`4m`, `0.004`: `parse_cpus` refuses
  it rather than returning 0, the server default); an out-of-range one fails
  planning with the validator's message. Either way nothing is written.
* **Omitted limits keep the current ones**: an existing service or
  datastore whose entry has no `plan`, `memoryLimit` or `cpuLimit` keeps
  its limits (like `numInstances` and `domains`: limits are often tuned
  outside the blueprint, and re-applying must not silently reset them); a
  new resource gets the server default. Limit changes appear in the
  `changes` of dry runs and applies: `memory_limit: 512 MiB → 2 GiB`,
  `cpu_limit: (server default) → 2 CPUs`.
* Apply order: env groups → datastores (create + provision) → services.
  Nothing is ever deleted. Existing resources are updated in place; the
  result lists `create` / `update` (with human-readable `changes`) /
  `unchanged`. After applying: new services with a source get a
  `blueprint` deploy; updated services whose build/deploy settings changed get
  a deploy; services whose resource limits (or cron command) changed but
  need no rebuild get a restart (trigger `restart`, when live or with a
  deploy in flight; a suspended live service gets a warning to resume and
  restart it instead);
  services whose only changes are env vars get a restart (if live).
  Existing datastores whose limits changed get action `update`: the row is
  written under the locks, then, once they are released,
  `engine.update_datastore_limits` applies them in place, whatever the
  datastore's status (§8; a failure becomes a warning). Blueprint applies
  also take the row locks of the existing datastores they name.
  Services with neither repo nor image produce a warning ("deploy with `ferry up <name>`").
* Blueprints don't mention git connections (§18): a service's `repo` is
  cloned with the connection of the server that serves it, like any other
  service's.
* `dry_run: true` computes the same result without writing anything.

## 12. CLI (`ferry`)

Config: `~/.config/ferry/config.json` `{server, token}` (0600). Overrides:
`--server/--token` flags, `FERRY_SERVER`/`FERRY_TOKEN` env. Global `--json`
prints raw API JSON. Tables are aligned plain text; colors only on a TTY and
never with `NO_COLOR`. Exit code 1 with the API error message on failure.

```
ferry login --server URL --token TOKEN     # verifies with /api/v1/info, saves config
ferry info                                 # incl. default limits and the Docker host's CPUs / memory
ferry services | ferry ls
ferry create NAME [--type web|pserv|worker|cron|static] [--repo URL] [--branch B] [--image IMG]
             [--runtime R] [--root-dir D] [--dockerfile P] [--build-cmd C] [--start-cmd C]
             [--publish-dir D] [--port N] [--health PATH] [--instances N] [--schedule CRON]
             [--disk MOUNT] [--domain D]... [--memory SIZE] [--cpu CPUS] [--env K=V]... [--env-group G]...
             [--no-auto-deploy] [--no-deploy] [--follow]
      # --memory 512M|1G|1.5G|<MiB>, --cpu 0.5|2|500m: limits of each instance and job run;
      # `default` (or 0) = the server default
ferry show NAME                                                     # incl. Memory limit / CPU limit
ferry update NAME [same flags as create, plus --auto-deploy/--no-auto-deploy]
      # --memory default / --cpu default go back to the server default; limits apply with the
      # next deploy or restart (the command prints which: `ferry restart NAME`, `ferry deploy NAME`, …)
ferry delete NAME [--yes]
ferry deploy NAME [--commit SHA] [--clear-cache] [--follow]        # follow = stream build logs, exit 1 on failure
ferry up [NAME] [--dir .] [--type web] [settings flags of create, incl. --memory/--cpu] [-e K=V]...
         [--env-group G]... [--clear-cache] [--follow]
      # tar.gz the directory (respect .gitignore/.ferryignore; skip .git, node_modules, target, .venv),
      # create the service if missing (type/runtime auto), upload → deploy; NAME defaults to dir name
ferry deploys NAME
ferry cancel DEPLOY_ID
ferry rollback NAME DEPLOY_ID
ferry restart NAME | ferry suspend NAME | ferry resume NAME
ferry scale NAME N
ferry status NAME                                                   # instances + cpu/mem against their limits;
      # a stopped instance shows why (`exited (exit code 1)`; `restarting (OOM killed, exit code 137)` when Docker's
      # OOMKilled flag is set — exit code 137 alone is not reported as OOM), with a note on stderr
      # ("Raise the limit with 'ferry update NAME --memory <SIZE>', then 'ferry restart NAME'.")
ferry logs NAME [-f] [--tail N] | ferry logs --deploy DEPLOY_ID [-f] | ferry logs --job JOB_ID [-f]
ferry env NAME                                                     # list
ferry env set NAME K=V... [--no-restart]  |  ferry env unset NAME K... [--no-restart]
ferry domains NAME | ferry domains add NAME DOMAIN | ferry domains rm NAME DOMAIN
ferry run NAME [--follow] [-- CMD...]                               # one-off job / trigger cron now
ferry jobs NAME
ferry db create NAME [--kind postgres|redis] [--version V] [--database D] [--user U] [--memory SIZE] [--cpu CPUS] [--wait]
ferry db ls | ferry db show NAME | ferry db rm NAME [--yes]
ferry db update NAME [--memory SIZE|default] [--cpu CPUS|default]  # at least one; applied to the running container, no restart
ferry env-group create NAME [K=V...] | ls | show NAME | set NAME K=V... | unset NAME K... | rm NAME
                | link SERVICE GROUP | unlink SERVICE GROUP
ferry blueprint apply [FILE] [--dry-run]       # FILE defaults to ./ferry.yaml, then ./render.yaml
ferry open NAME                                # print (and try to open) the service URL
```

Log output: `HH:MM:SS [instance] line`; system lines highlighted.

## 13. Web client (`web/`)

A single-page app with its own toolchain in `web/` (see `web/README.md`); the
Rust build never runs Node.

* **Stack:** React 19 + TypeScript (strict), Vite, Tailwind CSS v4, shadcn/ui
  (Radix primitives, customized in `src/components/ui`), lucide icons;
  server state in TanStack Query v5 (`src/lib/api/queries/*`), client state
  in Zustand (`src/stores/auth.ts`, `ui.ts`), routing with react-router v7
  (`createBrowserRouter`, history URLs, code-split pages); Vitest for the pure
  helpers (SSE parser, formatting, `.env` parsing, resource limits) and a few
  components (rendered with `react-dom/server`). Light and dark themes.
* **Views:** login (token, validated with `GET /api/v1/info`) · services
  (list, new) · service detail: Overview, Deploys (+ deploy detail with live
  build logs, rollback, cancel), Logs, Jobs (+ job detail), Environment
  (variables, env group links, "save & restart"), Settings (build & deploy,
  resources, custom domains, deploy hook) · datastores (+ detail,
  connection strings, resources) · env groups (+ detail) · blueprints (paste
  YAML → dry run → apply) · server (incl. default container limits, the
  Docker host's size, and the connected git accounts).
* **Git connections in the UI** (§18):
  * *Asking for one.* As long as the server has no connected account, the
    services page (the home page) asks to connect GitHub or GitLab — or to
    finish an authorization that was left half-way. It can be dismissed
    (per browser).
  * *Connecting.* The dialog's button sends the browser, in the same tab,
    to the provider's own pages: GitHub registers the server's app, then
    asks which repositories it may read; GitLab asks to authorize the
    server's application, whose Application ID and Secret the dialog takes
    the first time, next to the redirect URI and scopes to create it with.
    No token is typed; "Use an access token instead" is the dialog's other
    way. Options: a GitHub organization, the address of a self-hosted
    instance.
  * *Coming back.* The provider sends the browser to `/git/callback`, a
    page outside the app shell that hands the query parameters to `POST
    /api/v1/git/callback`, goes on to the provider's next page when there
    is one, and otherwise returns to where the user was (kept in
    `localStorage` for an hour; only ever a path of the app) with a toast —
    or says why the account was not connected.
  * *Picking a repository.* A new service's Git source is either a
    repository picked from a "Connected account" (the default) or a
    "Repository URL". The picker lists the repositories of the connected
    accounts with a search (private / archived badges; one without commits
    can't be picked); picking one fills in its default branch and, unless
    one was typed, the service name. Connecting an account from the form
    keeps the form: its draft waits in `sessionStorage` (never the
    credentials of a URL) and the new account is selected on return.
  * *Branches* are a searchable list read from the repository itself when
    the list opens (`GET /api/v1/git/branches`, the default branch first),
    for a picked repository as for a typed URL, in the new-service form and
    in Settings → Build & deploy; a name can still be typed (a branch that
    isn't pushed yet). The settings also say which account clones the
    repository.
  * *Managing.* Server → Connections → "Git accounts" lists the connections
    (how each was authorized, the services it clones for) with "Finish
    connecting", "Authorize again", "Repositories" (GitHub's page of the
    installation), "Replace token" and "Disconnect".
* **Resource limits in the UI** (§14): "Memory limit" / "CPU limit" selects
  (`Server default (512 MiB)` from `/api/v1/info`, presets, Custom…) in the
  service's Settings → Resources ("Changes apply on the next deploy or
  restart", then a "Restart now" toast), under Advanced when creating a
  service, in the new-datastore dialog, and in the datastore's Resources
  section (applied right away, no restart). `src/lib/resources.ts` mirrors
  `ferry_core::resources` (parsing, formatting, ranges); a new CPU value
  above the Docker host's CPUs is refused, memory above the host's is only
  flagged. Instance cards show each limit ("no limit" when unlimited) and,
  when Docker's `oom_killed` is set, an "OOM killed" badge linking to the
  settings.
* **Transport: REST + SSE** (no gRPC, no WebSocket). Queries and mutations
  are the JSON REST API of §10 on the same origin (no CORS), with
  `Authorization: Bearer`. Push uses Server-Sent Events read with `fetch()`
  and a streaming parser, so the token stays in a header: log streams
  (`event: log` … `event: end`) and the change feed `GET /api/v1/events`,
  whose `change` events become TanStack Query invalidations; it reconnects
  with exponential backoff (1 s → 15 s) and, while the feed is down (or 404
  on an older server), falls back to polling.
* **Storage:** the token in `localStorage` under `ferry.token`, UI
  preferences under `ferry.ui` (theme applied before first paint by
  `public/theme-init.js`, so a strict `script-src 'self'` CSP works). While
  a git account is being authorized: `ferry.git.authorizing` (where to go
  back) in `localStorage`, `ferry.new-service.draft` in `sessionStorage`;
  `ferry.git.prompt-dismissed` once the home page's prompt was dismissed.
* **Embedding:** `npm run build` writes `web/dist`; `ferry-api`'s `build.rs`
  embeds every file of it at compile time (`FERRY_WEB_DIST` overrides the
  directory; without a built client it embeds a placeholder page explaining
  how to build it). `ferryd` serves it at `/` (`ferry_api::web`): hashed
  `/assets/*` with `Cache-Control: public, max-age=31536000, immutable`,
  everything else (`index.html` first) `no-cache` + ETag; any other `GET`
  outside `/api`, `/hooks` and `/healthz` that isn't a file gets
  `index.html` (SPA fallback), a missing `/assets/` file stays a 404.
* **Development:** `FERRY_UI_DIR=<dir>` makes `ferryd` serve a built client
  from disk instead of the embedded one (rebuild the client without
  recompiling the server); or `npm run dev` in `web/` (Vite with HMR) proxies
  `/api`, `/hooks` and `/healthz` to `FERRY_API_URL` (default
  `http://127.0.0.1:7878`), SSE unbuffered.

## 14. Resource limits

Every container Ferry creates — service instances, one-off jobs, cron runs
and datastores; not `docker build` — runs with a memory limit, a CPU limit,
a pids limit and log rotation (unless the operator sets them to 0), so one
misbehaving app can't take the host down (§15).

**Data model.** `Service` and `Datastore` have `memory_limit_mb:
Option<u32>` (MiB) and `cpu_limit: Option<f64>` (CPUs, `0.5` = half a
core); `None` = the server default. Migration `0002_resource_limits.sql`
adds the nullable columns to `services` and `datastores`.
`Store::update_datastore` doesn't write them (it persists the engine-owned
columns, so the engine saving a stale row can never revert a limits
change); `Store::set_datastore_limits(id, memory, cpus)` does.
`ferry_core::resources` holds the accepted ranges (16 MiB to 1 TiB, 0.01 to
512 CPUs), `validate`, `round_cpus` (0.01), and the parsers / formatters
shared by the `ferryd` flags, the CLI and blueprints: `parse_memory_mb`
(`512`, `512M`, `1.5G`, `2GiB`; a plain number is MiB; `M`/`MB`/`MiB` all
mean MiB like in Docker), `format_memory_mb` (`512 MiB`, `1.5 GiB`; GiB
only for quarter-GiB multiples, exact at 2 decimals, so `1152 MiB` stays in
MiB and the text parses back — the web client's `formatMemoryMb` matches),
`parse_cpus` (`0.5`, `2`, `500m`; a non-zero amount that rounds to 0 is
an error), `format_cpus` (`0.5 CPU`, `2 CPUs`).

**Configuration** (`Config` field — `ferryd` flag / env, default):

| field | flag / env | default | meaning |
|---|---|---|---|
| `default_memory_limit_mb` | `--default-memory-limit` `FERRY_DEFAULT_MEMORY_LIMIT` | 512 (`512M`) | memory of each container whose service / datastore sets none; 0 = unlimited |
| `default_cpu_limit` | `--default-cpu-limit` `FERRY_DEFAULT_CPU_LIMIT` | 1.0 | CPU likewise; 0 = unlimited |
| `pids_limit` | `--pids-limit` `FERRY_PIDS_LIMIT` | 1024 | processes + threads per container (fork-bomb guard); 0 = unlimited |
| `log_max_size_mb` | `--log-max-size` `FERRY_LOG_MAX_SIZE` | 10 (`10M`) | `json-file` log driver with `max-size`, only when the daemon's default log driver is `json-file` (or unknown): another driver (journald, syslog, fluentd, `local`...) is the operator's choice and is kept; 0 = set no log config (the daemon's own applies) |
| `log_max_files` | `--log-max-files` `FERRY_LOG_MAX_FILES` | 3 | `max-file`, current file included (at least 1) |
| `min_free_disk_mb` | `--min-free-disk` `FERRY_MIN_FREE_DISK` | 1024 (`1G`) | free-disk check threshold; 0 = no check |

`ferryd` validates the flags as it parses them (a bad value stops startup
with an error naming the flag; a non-zero default must be a valid limit, and
a CPU default that rounds to 0, like `0.004`, is refused rather than meaning
unlimited), warns at startup when a default exceeds the Docker host's CPUs
or memory, and prints them in its banner: a `Limits` line (`512 MiB / 1 CPU
per container (default), 1024 pids, logs rotated at 10 MiB × 3`; `unlimited
memory`, `unlimited CPU`, `no pids limit`, `Docker's log settings` for the
settings that are 0, `Docker's journald log driver (kept)` when the daemon's
default driver isn't `json-file`), a `Free disk check` line (`deploys need 1 GiB free`
or `off`), and the host's CPUs and memory on the `Docker` line.
`--oom-score-adj` is not a `Config` field (§15).

**Effective limits** of a container = `config.limits(memory_limit_mb,
cpu_limit)`: the resource's own values, else the defaults (a default of 0 =
unlimited). The container spec gets `Memory` = `MemorySwap` = the memory
limit (no swap beyond it), `NanoCpus` = CPUs × 1e9 (not on a host without
CPU CFS quotas, see below), `PidsLimit` (unset when 0) and `LogConfig`
(only when `log_max_size_mb` > 0 and the daemon's default log driver is
`json-file`). Limits are per
container, not shared: a service with 3 instances may use 3 × its limit,
plus its running jobs.

**Services.** The limits apply to every instance and to the service's jobs
(one-off and cron runs). They are captured in the deploy's launch spec
(`LaunchSpec.memory_limit_mb` / `cpu_limit`, §5.6) when the deploy starts
its instances, so the reconciler, scale, resume and jobs use the live
deploy's limits. Changing them (API `PATCH`, `ferry update --memory`, the
dashboard) only saves them: they take effect with the next deploy or restart
(`ferry restart` reuses the live image, no rebuild). A blueprint apply is
different: it restarts every live service whose limits changed (§11) —
including, on the first re-apply of an existing `render.yaml` after
upgrading, every service whose `plan:` (ignored before) now maps to limits.
Launch specs stored before limits existed have none: their containers get
the server defaults when they are (re)created. Containers already running
when ferryd is upgraded keep running without limits until they are
replaced.

**Datastores.** Created with their limits; changed in place with
`PATCH /api/v1/datastores/{id}`, whatever the datastore's status (§8, §10);
provisioning that keeps an existing container applies the row's limits to
it when they differ. Redis gets `maxmemory` = 3/4 of its memory limit (§8).
Datastores created before limits existed keep running without them until
their limits are changed (PATCH), they are provisioned again (e.g. retried
after a failure) or their container is recreated: nothing is applied at
boot.

**Host capacity.** The engine reads `docker info` (CPUs, memory, Docker root
dir, OS, CPU CFS quota support, default log driver) on first use and caches it (5 s timeout; a failed read isn't
cached — nothing is capped then — and is retried next time). The cache is
never refreshed: after resizing Docker Desktop's VM, restart ferryd.
* Docker refuses a CPU quota above the host's CPU count ("range of CPUs is
  from 0.01 to N"), so the engine **caps** CPU limits at the host's CPUs and
  says so: `the CPU limit (8 CPUs) is more than the Docker host has: capped
  at 4 CPUs` (deploy log and server log; jobs: job log; datastores: server
  log).
* Docker accepts a memory limit above the host's memory; it just protects
  nothing, so it is only a warning (deploy log; jobs: job log; datastores:
  server log).
* On a host whose kernel has no CPU CFS quotas (`docker info`
  `CpuCfsQuota: false`: some NAS / ARM kernels, rootless Docker without the
  cgroup v2 `cpu` controller delegated) Docker refuses any container or
  update with `NanoCpus` ("NanoCPUs can not be set, as your kernel does not
  support CPU CFS scheduler"), while memory and pids limits only warn. So
  no CPU limit is set there — `docker update` leaves `NanoCpus` out too —
  and it is said: at ferryd startup (server log) and as `the CPU limit
  (1 CPU) is not enforced: the Docker host's kernel has no CPU CFS quota
  support` (deploy log and server log; jobs: job log; datastores: server
  log).
* `/api/v1/info` reports `docker_cpus` / `docker_memory_bytes` (read by
  `ferryd` at startup) so clients can check limits against the host.

**OOM visibility.** Docker's `OOMKilled` flag (inspect only — the list API
never reports it) is the proof of an out-of-memory kill; exit code 137 alone
is a SIGKILL, which may have other causes.
* Deploys: a new instance OOM-killed during its health check fails the
  deploy with `instance ab12cd ran out of memory (limit 512 MiB) — raise the
  service's memory limit` (§5.5), also when Docker already restarted it
  (the container's `oom` events tell).
* Jobs: `the job ran out of memory (limit 64 MiB) — raise the service's
  memory limit` (§7).
* Datastores: provisioning fails at once with `the redis container ran out
  of memory (limit 256 MiB) — raise the datastore's memory limit` (§8).
* Running containers: the OOM watcher (§6) logs a server-log warning for
  every kill.
* Status: `InstanceStatus` carries the container's configured
  `memory_limit_bytes` and `cpu_limit` (`None` = unlimited; not docker
  stats' limit, which reports the host's memory for unlimited containers),
  `oom_killed` and `exit_code` (filled for exited and restarting containers). Docker
  clears `OOMKilled` when a container runs again, so `oom_killed` is only
  seen while an instance is restarting or exited; past kills are in the
  server log.

**Free-disk check.** Before a Git, Archive or Image deploy builds or pulls
(§5.3) and before the container of a new datastore is created or a
datastore's image is pulled (§8; recreating an existing datastore's
container is not checked, so the reconciler can still heal it), the engine
checks the free space (`statvfs`: `f_bavail × f_frsize`, the space available
to unprivileged users; in `spawn_blocking`) of the filesystem of
`config.data_dir`, and of the Docker root dir when it is local (the path
exists on this machine and the daemon isn't Docker Desktop, whose root lives
in its VM). Below `min_free_disk_mb` the deploy fails (`build_failed`) or
the datastore goes `failed` with:

```
not enough free disk space on /srv/ferry: 812 MiB free, Ferry needs at least 1 GiB (free some space,
e.g. remove unused Docker images with `docker image prune`, or change the threshold with
`ferryd --min-free-disk`, 0 = off)
```

Paths that can't be checked are skipped (debug log); restarts and rollbacks
are never checked; on non-Unix systems the check does nothing.

## 15. Operational safety

* The data directory is created `0700` (it holds env values, datastore
  passwords, credentialed repo URLs, the secrets of git connections: access
  and refresh tokens, GitHub App private keys, OAuth client secrets);
  `api_token` and `instance_id` are `0600`.
* Git credentials (in a repository URL, or a git connection's token, §18)
  reach `git` through its environment only, never a command line, the git
  cache or a log, and only for the host of the remote: a host the remote
  redirects to gets none.
* The secrets of git connections never leave the server through the API
  (`GitConnection` is not serializable), a log or a `Debug` output. An
  answer of a provider is only accepted with the single-use `state` the
  server made up for it (§18).
* `ferryd` holds an exclusive lock on `<data-dir>/ferryd.lock`, and records
  ownership of its Docker name prefix on a marker volume `<prefix>-owner`.
  A server refuses to start on a prefix owned by another data dir (or when an
  empty data dir finds foreign containers) unless `--take-over` is given, so
  it can never reconcile away another server's workloads.
* The dashboard/API is only exposed through the public proxy by default for
  local base domains; on public domains it must be enabled explicitly
  (`--dashboard-host`), ideally with HTTPS.
* Shutdown: first SIGINT/SIGTERM drains (API ≤ 10s, then the engine stops
  jobs and records interrupted deploys, ≤ 25s); a second signal exits at once.
* Proxy: `X-Forwarded-For` is the peer address only (client-supplied
  forwarding headers are dropped; RFC 7239 `Forwarded` is set). Connection
  timeouts (TLS handshake, header read, idle) and a connection cap derived
  from the open-file limit (raised to the hard limit at startup).
* Env values may contain a literal `${{` written as `$${{`.

### One misbehaving service (with the default flags, §14)

| When a service… | what happens |
|---|---|
| leaks memory | the kernel OOM-kills its container at its memory limit (512 MiB by default; no swap beyond it) instead of picking some host process (ferryd, dockerd, a database). Docker restarts it (`unless-stopped`), the kill goes to the server log, and `ferry status` / the dashboard flag the instance while it restarts |
| spins the CPU | it is throttled at its CPU quota (1 CPU per container by default); the other cores stay free. On a 1-CPU host the default is the whole host: lower `--default-cpu-limit` there |
| forks without end | it stops at 1024 processes + threads per container (`fork` fails inside the container); the host's process table is untouched |
| logs without end | Docker's `json-file` log rotates at 10 MiB × 3 files: at most ~30 MiB per container (a daemon configured with another log driver keeps it, with that driver's own limits) |
| deploys onto a nearly full disk | the deploy (or a new datastore) fails before building or pulling, with a message naming the path; restarts and rollbacks still work |

Still **not** protected:
* **Builds.** `docker build` runs in BuildKit, which has no per-build memory
  or CPU limit: a build can still use all of the host's memory and CPU.
  Only `--build-concurrency` (default 2) bounds builds, and the free-disk
  check runs before a build, not during it.
* **Disks.** Volumes (datastores, service disks), container writable layers,
  images and the build cache have no quotas: a service writing to its disk
  or its container filesystem can fill the host's disk. The free-disk check
  only stops new deploys and new datastores; `--keep-images` bounds Ferry's
  own images, pruning the rest (`docker image prune`, `docker builder
  prune`) is up to the operator.
* **Disk and network I/O.** No I/O or bandwidth limits.
* **The network.** Every service and datastore shares one private network
  (§3): any container can reach every service and datastore by name, and
  datastores are protected by their passwords only.
* **Overcommit.** Limits are caps, not reservations: nothing checks that the
  sum of all limits (instances × limit, jobs, datastores) fits the host. When
  the host itself runs out of memory, the kernel's global OOM killer picks
  a victim — see ferryd's score below.
* **Unlimited by choice.** A limit of 0 (`--default-memory-limit 0`,
  `--default-cpu-limit 0`, `--pids-limit 0`, `--log-max-size 0`,
  `--min-free-disk 0`) turns that guard off.

**ferryd's OOM score** (Linux): at startup `ferryd` writes
`--oom-score-adj` (default `-500`, range -1000..=1000, `0` = leave
unchanged) to `/proc/self/oom_score_adj`, so that when the host runs out of
memory the kernel kills containers before ferryd. It never raises a lower
value already set (e.g. systemd's `OOMScoreAdjust=-900` stays). Lowering
the score needs root or `CAP_SYS_RESOURCE`: otherwise ferryd logs a warning
suggesting `OOMScoreAdjust=` in its systemd unit (and `--oom-score-adj 0` to
silence it). Processes ferryd starts (git, the docker CLI) don't keep a
lowered value: `ferry-build` resets their `oom_score_adj` to 0 before exec
(raising it needs no privilege), so a runaway `git index-pack` is not
spared at the expense of the containers and databases. Containers never
inherit it (dockerd starts them). On other systems the flag does nothing
(debug log). The README's systemd unit sets `OOMScoreAdjust=` for ferryd
(no `MemoryMin=`: a memory protection only takes effect when every
ancestor cgroup, e.g. `system.slice`, has one too).

## 16. Coding rules

* Rust 2024, stable. No `unwrap()`/`expect()` outside tests except on
  invariants documented in a comment. No `todo!()` left behind.
* Errors: `ferry_core::Error`; add context to messages ("pulling image nginx:
  …"). Log with `tracing` (`info` for lifecycle, `debug` for detail).
* Blocking work (tar, heavy fs) in `spawn_blocking`.
* Keep public signatures from the stubs. Private modules are yours: split
  `lib.rs` into modules freely.
* New dependencies: add them to **your own crate's** `Cargo.toml` with an
  explicit version (not to the workspace root).
* `cargo clippy -p <crate> --all-targets -- -D warnings` must pass; `cargo fmt`.

## 17. Testing

* Unit tests next to the code; integration tests in `crates/<crate>/tests/`.
* Docker-dependent tests are **gated**: they run only when `FERRY_E2E=1` and
  otherwise return early (print "skipped"). They must:
  * use a unique `name_prefix` (e.g. `ferrytest-<crate>-<random>`), unique
    free ports and a temp data dir, so tests from different crates can run
    concurrently against the same Docker daemon;
  * clean up everything they create, even on failure (containers, volumes,
    network, images tagged with their prefix):
    `docker ps -aq --filter label=ferry.instance=<prefix> | xargs docker rm -f`.
* Example apps in `examples/` (node-hello, python-hello, static-site,
  docker-echo, worker, blueprint) are the fixtures. To deploy one "from git",
  copy it to a temp dir and `git init && git add -A && git commit`.
* Small base images only in tests: `python:3.12-alpine`, `node:22-alpine`,
  `nginx:alpine`, `busybox:stable`, `postgres:16-alpine`, `redis:7-alpine`.
* Resource limits (§14) are checked against the real daemon:
  `ferry-docker/tests/docker_e2e.rs` (`limits_on_create`,
  `update_limits_in_place`, `oom_kills_are_reported`: inspect values and the
  kernel's cgroup files — skipped on cgroup v1 hosts —, log rotation, the
  pids limit, `docker update` and real OOM kills) and
  `ferry-engine/tests/e2e.rs`
  (`resource_limits_apply_to_instances_jobs_and_datastores`,
  `out_of_memory_kills_are_reported`).
* Git connections (§18) never call the real providers in tests:
  `ferry-api/tests/git_connections.rs` runs a fake GitHub Enterprise /
  GitLab on a local socket — accounts, paginated repositories, rejected
  and rate-limited tokens; the manifest conversion, installations and
  installation tokens of a GitHub App, checking the signature of every JWT;
  an OAuth token endpoint whose refresh tokens work once; and the
  repositories themselves over git's dumb HTTP protocol, behind those
  tokens. The GitHub App's RSA key is generated with the `openssl` CLI when
  the tests start (no key is kept in the repository; the app tests are
  skipped without `openssl`). `ferry-build` / `ferry-engine` clone and list
  branches from a local HTTP server that demands the token (also through a
  redirect, which must not get it).
* The background commands of `ferryd` (§19) are tested on the real binary
  in `ferryd/tests/background.rs`: without Docker, a start that fails
  before the server listens (its error reaches the terminal, exit code 1)
  and `status` / `stop` / `logs` when nothing runs; gated, the whole cycle
  `start` → `status` → `logs` → `stop`.

## 18. Git connections

A **git connection** is an account of a git provider — GitHub or GitLab,
their public instances or a self-hosted one — that this server is
authorized to read the repositories of. It does two things: the dashboard
lists the account's repositories, so a repository is picked instead of
typed, and every repository the connection serves is cloned with its
tokens, so private repositories deploy without credentials in their URL.

**Connections belong to the server, not to a service.** A service names no
connection: its `repo_url` decides which one clones it
(`git_connection_for`, below). An account is authorized once, for every
service — also those created later, by the CLI, a blueprint or the API.

**How an account authorizes the server** (`GitAuth`):

| `auth` | provider | what the user does | tokens |
|---|---|---|---|
| `github_app` | GitHub | confirms the registration of a GitHub App for this server, then installs it and chooses the repositories it may read | installation tokens (1 h), minted on demand with a JWT signed by the app's private key; kept in memory only |
| `oauth` | GitLab | authorizes an OAuth application created for this server | an access token (2 h), renewed with its refresh token |
| `token` | both | pastes a personal access token (the way without a browser) | the token as it is |

The first two happen on the provider's own pages: no token is typed, and
the server only gets read access (GitHub: `contents: read` and `metadata:
read` on the chosen repositories; GitLab: `read_api`, `read_repository`).
A GitHub App can be registered from a **manifest**, so no application has
to exist beforehand: it works for any server, whatever its address,
`localhost` included. GitLab has no such registration: its OAuth
application is created once by the user (redirect URI = the dashboard's
`/git/callback`, the two scopes, confidential) and its `client_id` /
`client_secret` are given to the server the first time; the instance's
next authorizations reuse them.

**Data model.** `GitConnection { id (git-…), provider, base_url, auth,
account, account_name, token, refresh_token, scopes, token_expires_at,
client_id, client_secret, app: GithubApp { id, slug, url, private_key,
webhook_secret }, installation: GithubInstallation { id, url,
repository_selection } }`; one row per `(provider, base_url, account)`,
the account compared case-insensitively. Migration
`0003_git_connections.sql` made the table for pasted tokens, with a
`git_connection_id` on services; `0004_git_authorization.sql` adds the
columns of the other kinds and drops that one, so the connections of a
database that ran the first become `token` connections and its services
are cloned with the connection of their repository's host. `base_url` is
the instance's web URL without a trailing slash (`https://github.com`,
`https://gitlab.com`, `https://gitlab.example.com`), validated by
`validate::git_base_url`. A
connection is **pending** until its authorization is finished
(`is_connected`): a GitHub App that is registered and not installed, an
OAuth application no account authorized yet (its `account` is empty).
Pending connections are listed, so the dashboard can finish them, and
never used. The type is not serializable: the API returns
`GitConnectionView` — `status` (`connected` / `pending`), `auth`, the
account, `client_id`, `app_slug` / `app_url`, `manage_url` (GitHub's page
of the installation, where its repositories are chosen),
`repository_selection` (`all` / `selected`), the names of the `services`
it clones for, and for a personal token `token_hint` (its end), `scopes`
and `token_expires_at` — and no secret.

**Authorizing in the browser** takes two calls, made by the dashboard:

1. `POST /api/v1/git/authorize` (`ferry_scm::start`) answers with the page
   to send the browser to. Its `redirect_uri` is the dashboard's
   `/git/callback` page (`validate::git_redirect_uri`: an http(s) URL
   without credentials, query string or fragment).
2. The provider sends the browser back to `redirect_uri` with query
   parameters, which that page hands to `POST /api/v1/git/callback`
   (`ferry_scm::callback`). It answers with the next page, or with the
   connection.

Every page the browser is sent to carries a `state` (48 hex characters)
the server made up and remembers in memory with what it expects back — for
an hour, at most 256 at a time — and an answer is only accepted with it,
once. A restart of the server forgets them: the authorization is started
again.

* **GitHub.** `authorize` answers `method: post`: the browser submits the
  form field `manifest` to `<base_url>/settings/apps/new?state=…` (or
  `/organizations/<organization>/settings/apps/new`, since a private app
  only reads the repositories of the account or organization that owns
  it). The manifest describes a private app named `ferry-<host>-<random>`
  with read access to contents and metadata, no webhook, and
  `redirect_url`, `setup_url` and `callback_urls` all set to
  `redirect_uri` (`setup_on_update`: GitHub also comes back after an
  installation's repositories were changed). GitHub redirects with
  `?code=`; the callback converts it (`POST
  /app-manifests/{code}/conversions`) into the app's id, slug, client id
  and secret and private key, saves them as the pending connection of the
  app's owner, and answers with `<app url>/installations/new?state=…`.
  GitHub redirects with `?installation_id=`; the callback reads that
  installation as the app (`GET /app/installations/{id}` — an id that
  isn't one of this app's is refused) and the connection is connected,
  under the account the app is installed on. An installation that waits
  for an organization owner's approval (`setup_action=request`) is told
  so; a suspended one is refused. A callback without `state` but with an
  `installation_id` (GitHub's own pages lead there) re-reads that
  installation, on the connection whose app has it.
* **GitLab.** `authorize` answers with `<base_url>/oauth/authorize?
  client_id&redirect_uri&response_type=code&state&scope=read_api+
  read_repository` — 400 `git_application_required` when the server has no
  application for that instance and the request brings none. An
  application given before any account authorized it is kept as a pending
  connection. GitLab redirects with `?code=`, or with
  `?error=access_denied` when the user refuses (400
  `git_authorization_rejected`); the callback exchanges the code (`POST
  <base_url>/oauth/token`, with the same `redirect_uri`), asks whose token
  it is (`GET /api/v4/user`) and saves the tokens on that account's
  connection.
* **`connection_id`** resumes or repeats an authorization: a GitHub App
  connection goes to its installation page, or is connected at once when
  the app turns out to be installed (`status: connected`); an OAuth
  connection is authorized again, with another application when the
  request brings one; a connection made with a token takes a new token
  instead. Connecting an account that is already connected, by any of the
  three ways, replaces what connected it.

**Tokens** (`ferry_scm::access`). A personal token is used as it is. An
OAuth access token is renewed with its refresh token 5 minutes before it
expires — one renewal at a time per connection, because GitLab's refresh
tokens work once — and both are saved (`Store::set_git_tokens`, which
leaves `updated_at` alone: a renewal is not a change). An installation
token is minted when needed (`POST /app/installations/{id}/access_tokens`,
authenticated by a JWT: RS256, `iss` = the app's id, expiring after 9
minutes, signed with `ring`), kept in memory until 5 minutes before its
hour ends, and never stored. When the authorization is gone — the refresh
token revoked, the app uninstalled or deleted — the error says to connect
the account again.

**With a token** (`POST /api/v1/git/connections`): the token is checked by
asking the provider whose it is — GitHub `GET /user` (`api.github.com`, or
`<base_url>/api/v3` on GitHub Enterprise Server), GitLab `GET
<base_url>/api/v4/user` — always as `Authorization: Bearer`. The answer
gives the account, and what is known about the token: its scopes (GitHub's
`x-oauth-scopes`, for classic tokens; GitLab's `GET
/personal_access_tokens/self`) and its expiry
(`github-authentication-token-expiration`; `expires_at`). Needed: on GitHub
a classic token with `repo` (or a fine-grained one with read access to
Contents and Metadata), on GitLab `read_api` + `read_repository`.

**Which connection clones a repository** (`git_connection_for`).
`GitConnection::serves(repo_url)`: an http(s) URL of the same scheme, host
and port as `base_url` (below its path when it has one), read by
`git::parse_http_url`, which refuses anything that isn't a plain URL:
credentials, unusual characters, other schemes. A URL with credentials of
its own, an ssh URL and a local path are never served, so a token is never
sent anywhere but to its own provider. Among the connected connections
that serve the URL, the one of the repository's owner
(`https://github.com/<account>/…`) comes first, then the most recently
updated. A GitHub App is only used for the account it is installed on (its
tokens read nothing else); a user's OAuth or personal token is tried for
any repository of its instance.

**Listing repositories** (`GET …/connections/{id}/repositories`). A GitHub
App: `GET /installation/repositories`, exactly what the account allowed.
Otherwise GitHub `GET /user/repos?sort=pushed` (owned, collaborator and
organization repositories), GitLab `GET
/projects?membership=true&order_by=last_activity_at`. 100 per page, at
most 10 pages (`truncated` says when there are more), the pages after the
first fetched 4 at a time when the provider says how many there are.
Nothing is cached on the server; the dashboard keeps a listing for a
minute and has a Refresh button.

**Branches** (`GET /api/v1/git/branches?repo_url=`) are asked from the
repository itself, not from the provider's API: `git ls-remote --symref
<url> HEAD 'refs/heads/*'` (`ferry_build::remote_branches`, 20 s), with
what a deploy would clone with — the connection that serves the URL, the
credentials the URL carries, or nothing. It therefore works for any URL a
service can deploy from (other hosts, ssh, a path on the server) and shows
what a deploy will see. The answer has the default branch (the remote's
`HEAD`) first, then the others by name, and `connection_id` when a
connection was used. A remote that refuses, isn't found or doesn't answer
is 502 `git_remote_unreachable`, with the hint of the next paragraph.

**Cloning** (§5.3). The pipeline asks `ferry_scm::repo_access` what reads
the deploy's `repo_url`, logs `==> Cloning with the GitHub account
'octocat'` and hands `BuildSource::Git` the credentials: the token as the
password, with the username `x-access-token` (GitHub) or `oauth2`
(GitLab). `ferry-build` gives them to git like credentials embedded in a
URL (§15). A connection that can't produce a token is said (`==> Warning:
the … can't be used (…): cloning without it`) and the clone goes on
without it: a public repository still deploys. When the remote refuses the
clone, the deploy log gets a `==> Hint:` — add the repository to the app's
installation; check the account's access or connect it again; the token
may have expired; or, without a connection, that a private repository
needs its account connected to the server. A deploy's `source` and the git
cache only ever hold the plain URL.

**Provider failures** never answer 401 (which means "wrong Ferry token" to
API clients: the dashboard signs out). An authorization the provider
refuses is 400 while it is being made (a rejected token or code, a user
who said no) and 409 when it is what a connection has stored (also a
pending connection asked for its repositories) — both with code
`git_authorization_rejected` — and an unreachable, failing or
rate-limiting provider is 502 `git_provider_unavailable`. Messages name
the provider and never contain a secret.

**Removing.** `DELETE /api/v1/git/connections/{id}` is refused (409) while
the connection clones the repository of services, unless `force=true`;
those services keep their repository and are cloned without credentials
from then on (or with another connection that serves it). What was
authorized stays on the provider until it is removed there: the GitHub App
(in GitHub's settings), the OAuth grant, the token.

**Change feed.** Connections are the `git_connection` kind of §10's feed,
fingerprinted by `id` and `updated_at` only: a renewed token is not a
change.

Not covered: registering webhooks on the provider (the GitHub App has
none; auto-deploy still needs the GitHub webhook of §10 or a deploy hook),
encrypting the stored secrets beyond the data directory's permissions
(§15), and connecting accounts from the CLI or a blueprint (both deploy
repositories through the server's connections, like the API).

## 19. Running the server (`ferryd`)

`ferryd` is one binary: the server's options (`ferryd --help`) and five
commands.

| command | does |
|---|---|
| `ferryd [options]` | `start` when stdin and stdout are a terminal (and it is not PID 1, Unix only), `run` otherwise: a service manager, a container, a pipe and a script get the foreground server they supervise |
| `ferryd start [options]` | starts the server in the background and returns once it listens |
| `ferryd run [options]` | the server itself, in the foreground, until SIGINT / SIGTERM (a second one exits at once) |
| `ferryd stop` | SIGTERM to the server of the data directory, then waits (up to 60 s) until it released the lock; a second `stop` is the server's second signal: it exits at once |
| `ferryd status` | where the server of the data directory listens; exit code 0 when one runs, 3 when none does |
| `ferryd logs [-f] [-n N]` | the end (100 lines) of `ferryd.log`, and with `-f` what is appended to it |

Options are written after the command (`ferryd run --data-dir …`); one in
front of a command is an error, not ignored. `stop`, `status` and `logs`
only take `--data-dir` / `FERRY_DATA_DIR`.

**Finding the server** takes two files of the data directory. The lock
`ferryd.lock` (§15) says whether a server is alive: nothing else is trusted
for that. `ferryd.json` (`0600`) says what it is — `ServerState { pid,
version, started_at, background, ready: { api_url, summary }? }` — written
by every server (`run` too) when it has the lock, completed with `ready`
(the lines of the startup banner) when the API listens, and removed when
it stops. A `ferryd.json` without the lock held is a leftover of a server
that was killed and is ignored; a lock held without the file (a server of
an older version) is "running" for `status` and an error for `stop`, which
never signals a process it can't name.

**`start`** (`ferryd::daemon::start`, Unix only — elsewhere it says to use
`ferryd run`):

1. A server already holds the data directory: prints where it listens and
   exits 0, changing nothing.
2. Opens `<data-dir>/ferryd.log` for appending (`0600`), after moving a
   file bigger than 10 MiB to `ferryd.log.1`.
3. Executes itself as `ferryd run --detached <the same options>` — the
   command line as typed, so relative paths and the environment mean the
   same — with stdin from `/dev/null`, stdout and stderr to the log, in a
   session of its own (`setsid`): closing the terminal (SIGHUP) and Ctrl-C
   in it no longer reach the server.
4. Polls, every 50 ms: the child exited → prints what it logged (its
   `Error: …` included) and exits 1; `ferryd.json` has the child's pid and
   `ready` → prints the banner under "running in the background (pid N)",
   the warnings and errors it logged while starting, and how to follow and
   stop it; after 60 s → exits 1, saying the server keeps starting.

The server logs with colors only when its stdout is a terminal (and
`NO_COLOR` is unset): never into `ferryd.log`, a pipe or the journal.
Nothing rotates `ferryd.log` while a server runs; a long-lived server
belongs under a service manager with `ferryd run` (README, "Running the
server").

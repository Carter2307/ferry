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
| `ferry-core` | lib | — | models, DTOs, config, `Store` (SQLite), env resolution, naming, `Engine` trait, `TlsHooks` trait, validation, cron schedules, git URL helpers. **Frozen.** |
| `ferry-docker` | lib | core | typed Docker wrapper (containers, images, volumes, networks, logs, stats, exec) |
| `ferry-build` | lib | core | git fetch / archive extract, runtime detection, Dockerfile generation, `docker build` |
| `ferry-proxy` | lib | core | `RouteTable`, HTTP/HTTPS reverse proxy, websockets, error pages |
| `ferry-tls` | lib | core | ACME certificates, SNI resolver, `TlsHooks` impl |
| `ferry-engine` | lib | core, docker, build, proxy | `FerryEngine: Engine` — deploys, reconciler, cron, jobs, datastores, logs |
| `ferry-api` | lib | core | axum router: REST, SSE (logs + `/api/v1/events`), webhooks, blueprints, OpenAPI + Swagger UI, serves the embedded web client (`web/dist`) |
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
3. **Build** (`building`): Git/Archive → `Builder::build` with image tag
   `naming.image_tag(name, deploy_id)`, build args = the service's resolved env,
   labels `naming.service_labels`. Image → `docker.ensure_image` (pull; always
   pull when the tag is `latest`/untagged). Reuse → verify the image exists.
   Record `image`, `commit_sha`, `commit_message` on the deploy (the commit is
   recorded as soon as it is checked out — `BuildEvent::CheckedOut` — so failed
   builds show it too). Pulled images are **pinned**: tagged locally as
   `naming.image_tag(name, deploy_id)`, which becomes `deploy.image` (the
   user's reference stays in `source.image`), so rollbacks are exact even when
   `:latest` moves. Build-time env reaches builds as **BuildKit secrets**
   (`--secret id=KEY,env=KEY`; generated Dockerfiles mount them per `RUN`),
   never as `ARG`s, so values don't end up in the image history.
   Cron jobs stop here: mark `live`, set `live_deploy_id`, deactivate previous.
4. **Start** (`deploying`): port = `env::choose_port(service.port, user_env,
   build.port_hint, docker.image_exposed_ports(image), config.default_port)`
   for listening services (store it in `deploy.port`); env = `env::container_env(
   env::injected(..), env::resolve_all(store.effective_env, RefContext))`;
   `cmd = ["/bin/sh","-c", start_command]` when a start command is set and the
   runtime is `docker`/`image` (generated Dockerfiles already bake it in);
   restart policy `UnlessStopped`; network + alias; disk volume if configured.
   Start `desired_instances()` new containers.
   * **Services with a disk** use *recreate*: stop + remove old containers first.
   * Everything else is *blue/green*: old containers keep serving.
5. **Health check** each new instance, polling every 1s up to
   `config.health_check_timeout_secs`:
   * container exited/dead → fail immediately, copy its last 50 log lines into
     the deploy log;
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
   disk) is **snapshotted** when it goes live. The reconciler, scale, resume
   and one-off jobs always start instances from the live deploy's snapshot —
   never from current settings/env — so settings and env changes take effect
   only through a deploy or restart (like Render), and a failed env-change
   deploy can't break self-healing.
7. **Failure**: remove new containers, keep old ones serving, status
   `build_failed`/`deploy_failed` with a concise `error`.
8. **Cleanup**: keep the newest `config.keep_images` images of this service
   (never the live one; only images whose repo is `naming.image_repo(name)` —
   never delete pulled public images); delete scratch dirs.
9. **Cancel**: queued → canceled at once; building → cancel token (builder
   kills `docker build`); deploying → remove new containers. Terminal → Conflict.

Log conventions (deploy log): system lines start with `==> ` (e.g.
`==> Cloning from https://github.com/a/b (branch main)`,
`==> Using Dockerfile at ./Dockerfile`, `==> Build successful 🎉`,
`==> Starting 2 instance(s)`, `==> Health check passed`, `==> Your service is live 🎉`).

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
* datastores: ensure container exists & running (volumes keep data).
* orphans: containers with our `ferry.instance` label whose service/datastore
  no longer exists → remove. Job containers of finished/unknown jobs → remove.
* boot only: deploys still `queued/building/deploying` → `build_failed` /
  `deploy_failed` with error "interrupted by server restart"; jobs still
  `pending/running` → `failed`.

## 7. Jobs & cron

* Scheduler ticks every ~15s; for each non-suspended cron job with a live
  deploy, `Schedule::fires_between(last_tick, now)` → `run_job(Schedule)`.
  Missed runs while the server was down are skipped. If a previous run of the
  same service is still running, skip (log at info).
* A job run: container `naming.job_container`, labels `naming.job_labels`,
  image = live deploy image, cmd = `sh -c <command>` (override, else service
  start command, else image default), env like the service (no `PORT`),
  network joined (reaches datastores / private services), no published port,
  restart `No`. Output streamed into `logs/jobs/<id>.log`; status `running` →
  `succeeded` (exit 0) / `failed`; container removed after.
* `run_job` on non-cron services requires `command`; needs a live deploy.
  One-off jobs mount the service's disk.

## 8. Datastores

* Postgres: image `postgres:{version}-alpine`, env `POSTGRES_USER`,
  `POSTGRES_PASSWORD`, `POSTGRES_DB`; volume at `/var/lib/postgresql/data`;
  ready when `pg_isready -U <user> -d <db>` (exec) exits 0.
* Redis: image `redis:{version}-alpine`, cmd `redis-server --requirepass <pw>
  --appendonly yes`; volume at `/data`; ready when `redis-cli -a <pw> ping` → `PONG`.
* Publish internal port on `datastore_bind_ip:host_port`; alias = name;
  restart `UnlessStopped`. Status `creating → available | failed` (error set).
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
| `GET /api/v1/info` | `ServerInfo` |
| `GET /api/v1/events` | SSE change feed: `event: ready` (`data: {}`) once the feed watches the store (refetch after it), then `event: change` with `ChangeEvent` `{kind, id, service_id, action}` — `kind` ∈ `service`/`deploy`/`datastore`/`env_group`/`job`, `action` ∈ `created`/`updated`/`deleted`, `service_id` set for services (own id), deploys and jobs; a lagging subscriber gets `{kind:"all", id:"*", service_id:null, action:"resync"}` (refetch everything). The store is polled every second while someone listens and nudged after every API/webhook write |
| `GET /api/v1/services` | `[ServiceView]` |
| `POST /api/v1/services` | `CreateService` → 201 `ServiceView` (queues a `create` deploy when it has a repo/image, unless `deploy:false`) |
| `GET /api/v1/services/{id}` | `ServiceView` |
| `PATCH /api/v1/services/{id}` | `UpdateService` → `ServiceView` (instances → `engine.scale`, suspended → `engine.suspend/resume`, custom_domains → `engine.refresh_routes`; the rest is stored) |
| `DELETE /api/v1/services/{id}?force=` | 204 (`engine.delete_service`); 409 listing the referencing services when other services reference it via `${{service.…}}`, unless `force=true` |
| `GET /api/v1/services/{id}/status` | `RuntimeStatus` |
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
| `POST /api/v1/datastores` | `CreateDatastore` → 201 `DatastoreView` (row `creating`, then `engine.provision_datastore`) |
| `GET /api/v1/datastores/{id}` · `DELETE ?force=` | `DatastoreView` · 204 (409 while referenced, unless `force=true`) |
| `GET /api/v1/env-groups` · `POST` | `[EnvGroupView]` · `CreateEnvGroup` → 201 `EnvGroupView` |
| `GET /api/v1/env-groups/{id}` · `DELETE ?force=&restart=` | `EnvGroupView` · 204 (409 while linked, unless `force=true`; `restart=true` restarts the linked live services) |
| `PUT` / `PATCH /api/v1/env-groups/{id}/env?restart=` | `ReplaceEnv` / `PatchEnv` → `EnvGroupView` (restart linked live services) |
| `POST /api/v1/blueprints/apply` | `ApplyBlueprint` JSON, or raw YAML (`Content-Type: application/yaml` / `text/yaml`, `?dry_run=`) → `BlueprintResult` |
| `GET\|POST /hooks/deploy/{service_id}?key=` | 202 minimal deploy summary (trigger `deploy_hook`, credentials redacted); service **id** only; unknown id or wrong key → the same 401 (no enumeration) |
| `POST /hooks/github` | GitHub webhook. 404 unless `github_webhook_secret` set; verify `X-Hub-Signature-256` (HMAC-SHA256, constant-time) → 401; `ping` → 200; `push` → deploy (trigger `webhook`, commit = `after`) every service with `auto_deploy`, matching `git::normalize_repo_url` of `repository.clone_url`/`ssh_url`/`html_url`, and `refs/heads/<branch>`; deleted-branch pushes ignored → 200 `{"deploys": [...]}` |

Name rules: `validate::resource_name` for services/datastores (names shared
between both; id-shaped names are rejected), `validate::env_group_name`,
`validate::env_vars` (32 KiB per value, 256 KiB per owner, also checked on
the merged service + linked groups). Request bodies reject unknown fields.
Git inputs are validated up front (`validate::repo_url` — absolute local
paths only —, `branch`, `commit`). Switching a service between git and image
requires clearing the other source in the same PATCH. Cron jobs always have
exactly 1 instance. Writes to one service are serialized, and custom-domain
claims are globally serialized. GitHub webhooks also accept form-encoded
deliveries and ignore replayed payloads.

## 11. Blueprints (`ferry.yaml` / `render.yaml`)

Render's schema (camelCase), a pragmatic subset:

```yaml
envVarGroups:            # [{name, envVars: [{key, value} | {key, generateValue: true}]}]
databases:               # Postgres: {name, databaseName?, user?, postgresMajorVersion?}
services:
  - type: web|pserv|worker|cron|static|redis|keyvalue   # web + runtime: static → static site
    name: api
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
* Unknown / unsupported keys (`plan`, `region`, `scaling`, `previews`, …) are
  ignored with a warning, never an error.
* Apply order: env groups → datastores (create + provision) → services.
  Nothing is ever deleted. Existing resources are updated in place; the
  result lists `create` / `update` (with human-readable `changes`) /
  `unchanged`. After applying: new services with a source get a
  `blueprint` deploy; updated services whose build/deploy settings changed get
  a deploy; services whose only changes are env vars get a restart (if live).
  Services with neither repo nor image produce a warning ("deploy with `ferry up <name>`").
* `dry_run: true` computes the same result without writing anything.

## 12. CLI (`ferry`)

Config: `~/.config/ferry/config.json` `{server, token}` (0600). Overrides:
`--server/--token` flags, `FERRY_SERVER`/`FERRY_TOKEN` env. Global `--json`
prints raw API JSON. Tables are aligned plain text; colors only on a TTY and
never with `NO_COLOR`. Exit code 1 with the API error message on failure.

```
ferry login --server URL --token TOKEN     # verifies with /api/v1/info, saves config
ferry info
ferry services | ferry ls
ferry create NAME [--type web|pserv|worker|cron|static] [--repo URL] [--branch B] [--image IMG]
             [--runtime R] [--root-dir D] [--dockerfile P] [--build-cmd C] [--start-cmd C]
             [--publish-dir D] [--port N] [--health PATH] [--instances N] [--schedule CRON]
             [--disk MOUNT] [--domain D]... [--env K=V]... [--env-group G]... [--no-auto-deploy]
             [--no-deploy] [--follow]
ferry show NAME
ferry update NAME [same flags as create, plus --auto-deploy/--no-auto-deploy]
ferry delete NAME [--yes]
ferry deploy NAME [--commit SHA] [--clear-cache] [--follow]        # follow = stream build logs, exit 1 on failure
ferry up [NAME] [--dir .] [--type web] [--create-only]... [--follow]
      # tar.gz the directory (respect .gitignore/.ferryignore; skip .git, node_modules, target, .venv),
      # create the service if missing (type/runtime auto), upload → deploy; NAME defaults to dir name
ferry deploys NAME
ferry cancel DEPLOY_ID
ferry rollback NAME DEPLOY_ID
ferry restart NAME | ferry suspend NAME | ferry resume NAME
ferry scale NAME N
ferry status NAME                                                   # instances + cpu/mem
ferry logs NAME [-f] [--tail N] | ferry logs --deploy DEPLOY_ID [-f] | ferry logs --job JOB_ID [-f]
ferry env NAME                                                     # list
ferry env set NAME K=V... [--no-restart]  |  ferry env unset NAME K... [--no-restart]
ferry domains NAME | ferry domains add NAME DOMAIN | ferry domains rm NAME DOMAIN
ferry run NAME [--follow] [-- CMD...]                               # one-off job / trigger cron now
ferry jobs NAME
ferry db create NAME [--kind postgres|redis] [--version V] | ferry db ls | ferry db show NAME | ferry db rm NAME [--yes]
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
  helpers (SSE parser, formatting, `.env` parsing). Light and dark themes.
* **Views:** login (token, validated with `GET /api/v1/info`) · services
  (list, new) · service detail: Overview, Deploys (+ deploy detail with live
  build logs, rollback, cancel), Logs, Jobs (+ job detail), Environment
  (variables, env group links, "save & restart"), Settings (build & deploy,
  custom domains, deploy hook) · datastores (+ detail, connection strings) ·
  env groups (+ detail) · blueprints (paste YAML → dry run → apply) · server.
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
  `public/theme-init.js`, so a strict `script-src 'self'` CSP works).
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

## 14. Operational safety

* The data directory is created `0700` (it holds env values, datastore
  passwords, credentialed repo URLs); `api_token` and `instance_id` are `0600`.
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

## 15. Coding rules

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

## 16. Testing

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

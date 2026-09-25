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
| **Interfaces** | web dashboard, `ferry` CLI, REST API, `ferry.yaml` / `render.yaml` blueprints |

## Quick start

Requirements: Docker (Docker Desktop on macOS works), Rust 1.85+ to build.

```bash
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

Open the dashboard at **http://127.0.0.1:7878** (or http://ferry.localhost:8080).

### More examples

```bash
# From git, with 2 instances and a health check
ferry create api --repo https://github.com/you/api --branch main --instances 2 --health /healthz --follow

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
   ```

3. Keep the API on localhost and reach it over SSH, or expose the dashboard
   through the proxy at `ferry.apps.example.com`.
4. For GitHub auto-deploys, add a webhook to your repo:
   `https://ferry.apps.example.com/hooks/github`, content type JSON, with the
   same secret.

### Server configuration

| Flag / env | Default | Meaning |
|---|---|---|
| `--data-dir` `FERRY_DATA_DIR` | `./ferry-data` | database, logs, build scratch, certs |
| `--api-addr` `FERRY_API_ADDR` | `127.0.0.1:7878` | API + dashboard |
| `--proxy-addr` `FERRY_PROXY_ADDR` | `0.0.0.0:8080` | public HTTP |
| `--https-addr` `FERRY_HTTPS_ADDR` | – | public HTTPS (with `--acme-email`) |
| `--base-domain` `FERRY_BASE_DOMAIN` | `localhost` | apps live at `<name>.<base-domain>` |
| `--acme-email` `FERRY_ACME_EMAIL` | – | enables Let's Encrypt |
| `--github-webhook-secret` | – | enables `/hooks/github` |
| `--api-token` `FERRY_API_TOKEN` | generated | stored in `<data-dir>/api_token` |
| `--name-prefix` `FERRY_NAME_PREFIX` | `ferry` | Docker resource prefix (run several Ferrys on one daemon) |
| `--build-concurrency` | `2` | parallel builds |
| `--health-timeout` | `120` | seconds for a deploy to become healthy |

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

## Development

```bash
cargo test --workspace                     # unit + API tests
FERRY_E2E=1 cargo test --workspace         # + Docker end-to-end tests
cargo clippy --workspace --all-targets -- -D warnings
```

Crates: `ferry-core` (models, store, contracts) · `ferry-docker` · `ferry-build` ·
`ferry-proxy` · `ferry-tls` · `ferry-engine` · `ferry-api` (+ dashboard) ·
`ferry-cli` (`ferry`) · `ferryd`.

## License

MIT

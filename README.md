# Relais

> The open-source agent internet gateway. Turn any website into a structured CRUD API for AI agents.

*Relais* (French: relay station) -- a digital relay station where AI agents access the internet.

## What is Relais?

Relais sits between your AI agents and the internet. One JWT, one entry point -- your agents get a unified API to read, write, and delete data across any website.

- **Native Adapters** -- High-quality, community-contributed adapters for popular sites (GitHub, Hacker News, ...)
- **LLM Fallback** -- For sites without adapters, Relais uses an LLM to extract structured data from raw HTML
- **Encrypted Vault** -- Site credentials stored with AES-256-GCM encryption
- **Self-hostable** -- Run it on your own infrastructure, no vendor lock-in

## Quick Start

### Install

```bash
cargo install relais-cli
```

### Usage

```bash
# List available sites
relais sites

# Explore a site's API
relais apis github
relais spec github.repos.list

# Execute an action
relais exec github.repos.list --data '{"owner": "rust-lang"}'

# Manage credentials
relais vault store github ghp_your_token_here
relais vault list

# Start the HTTP API server
relais serve --port 3000
```

### HTTP API

```bash
# Get a JWT (for development)
# JWT_SECRET defaults to "dev-secret"

# List sites
curl -H "Authorization: Bearer $TOKEN" http://localhost:3000/v1/sites

# Execute an action
curl -X POST http://localhost:3000/v1/exec \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"site": "hackernews", "resource": "stories", "action": "list_top", "params": {}}'
```

## Architecture

```
 ┌──────────────────────────────────────────────────────────────┐
 │                        AI  Agent                             │
 │                     (any framework)                          │
 └────────────────┬─────────────────────────────────────────────┘
                  │  JWT-authenticated HTTP
                  ▼
 ┌──────────────────────────────────────────────────────────────┐
 │                      Relais Gateway                          │
 │                                                              │
 │  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐  │
 │  │   Router     │  │  Auth (JWT)  │  │  Credential Vault │  │
 │  │  /v1/exec    │  │  HS256 verify│  │  AES-256-GCM      │  │
 │  └──────┬───────┘  └──────────────┘  └───────────────────┘  │
 │         │                                                    │
 │  ┌──────▼────────────────────────────────────────────────┐   │
 │  │                   Adapter Layer                       │   │
 │  │                                                       │   │
 │  │  ┌─────────┐  ┌──────────────┐  ┌─────────────────┐  │   │
 │  │  │ GitHub  │  │ Hacker News  │  │  LLM Fallback   │  │   │
 │  │  │ Adapter │  │   Adapter    │  │  (any website)  │  │   │
 │  │  └────┬────┘  └──────┬───────┘  └───────┬─────────┘  │   │
 │  └───────┼──────────────┼───────────────────┼────────────┘   │
 └──────────┼──────────────┼───────────────────┼────────────────┘
            │              │                   │
            ▼              ▼                   ▼
      GitHub API     HN Firebase API    Headless Browser
                                         + LLM Provider
```

## Crate Structure

| Crate | Description |
|-------|-------------|
| `relais-core` | Adapter trait, resource tree types, router, encrypted vault |
| `relais-server` | Axum HTTP API with JWT auth middleware |
| `relais-cli` | CLI binary (`relais`) with all subcommands |
| `relais-llm-fallback` | Headless browser fetch + multi-provider LLM extraction |
| `relais-adapter-github` | Native GitHub adapter (repos, issues, comments) |
| `relais-adapter-hackernews` | Native Hacker News adapter (stories, comments, users) |
| `relais-adapter-scs-legacy` | Legacy SCS adapter — full `/1/*` API (79 modules, 1324 endpoints), generated from Swagger; site `scs` |
| `relais-adapter-scs` | SCS kratos-rewrite adapter (accounts); site `scs-v2` |

## Writing Adapters

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to write and submit adapters.

## Agent Skills

The [`skills/`](skills/) directory ships loadable skills that teach an AI agent
to drive relais for a specific domain. They are distributables (relais itself
does not load them) — a third party can drop one into their agent's skills
directory (e.g. a Claude Code `skills/` folder) and the agent can immediately
operate the corresponding site through relais.

- [`skills/scs-legacy`](skills/scs-legacy/SKILL.md) — operate the full SCS (娱集市后台) platform (79 modules) via relais.
- [`skills/scs-accounts`](skills/scs-accounts/SKILL.md) — manage SCS accounts on the kratos `scs-v2` service via relais.

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).

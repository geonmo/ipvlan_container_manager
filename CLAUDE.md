# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Rust web application that generates configuration files for 2-node HA container clusters on RHEL9/AlmaLinux9. The stack combines DRBD (block replication) + Quadlet (Podman systemd containers) + Pacemaker (cluster orchestration).

## Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build
cargo run                # Run server (listens on 0.0.0.0:5000)
cargo test               # Run tests
cargo check              # Fast compile check without producing binary
cargo clippy             # Lint
cargo fmt                # Format code
RUST_LOG=debug cargo run # Run with debug logging
```

## Architecture

### Three-Module Pipeline

The app guides users through a sequential 3-step workflow:

1. **DRBD** → generates `.res` resource file + Ansible inventory
2. **Quadlet** → generates systemd `.container`/`.network`/`.pod`/`.volume` unit files
3. **Pacemaker** → generates `pcs` bash script + Ansible playbooks for cluster setup

### Code Layout

Each of the three subsystems has a parallel structure:

| Layer | Purpose |
|-------|---------|
| `src/models/{drbd,quadlet,pacemaker}.rs` | Data structures deserialized from HTML form POST bodies |
| `src/generators/{drbd,quadlet,pacemaker}.rs` | Pure functions: model → file content strings |
| `src/routes/{drbd,quadlet,pacemaker}.rs` | Axum handlers: parse form → call generator → render template |
| `templates/{drbd,quadlet,pacemaker}/` | Tera templates (Jinja2 syntax) for form input and result display |

`src/main.rs` wires up the Axum router, Tera template engine, and tower-http static file serving.

### Key Design Points

- **No database** — all state is in the HTTP request/response cycle. Each form POST is stateless.
- **Quadlet "from-inspect" path**: `POST /quadlet/from-inspect` accepts raw `podman inspect` JSON and calls `podman_inspect_to_quadlet()` in `src/generators/quadlet.rs` to auto-generate unit files without manual form entry.
- **Default Pacemaker constraints**: `generate_default_constraints()` auto-creates order and colocation constraints so Quadlet services only start after DRBD is promoted on the same node.
- Templates are loaded at startup from `templates/**/*.html` (not hot-reloaded in release builds).
- Static assets served from `static/` via tower-http.

### HTTP Routes

```
GET  /                    → Home/overview
GET  /drbd/               → DRBD form
POST /drbd/generate       → Generate DRBD .res file + Ansible
POST /drbd/download       → Download .res file

GET  /quadlet/            → Quadlet form
POST /quadlet/generate    → Generate Quadlet unit files
POST /quadlet/from-inspect → Parse podman inspect JSON → Quadlet units

GET  /pacemaker/          → Pacemaker form
POST /pacemaker/generate  → Generate pcs script + Ansible
```

### Supporting Files

- `scripts/podman_to_quadlet.sh` — bash alternative to the web UI; converts live Podman containers via `podman inspect` + `jq`
- `ansible/playbooks/` — three playbooks (deploy_drbd.yml, deploy_quadlet.yml, deploy_pacemaker.yml) that are embedded in the generated output and shown to users

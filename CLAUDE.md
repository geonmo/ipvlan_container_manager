# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Rust web application that generates configuration files for 2-node HA container clusters on RHEL9/AlmaLinux9. The stack combines DRBD (block replication) + Quadlet (Podman systemd containers) + Pacemaker (cluster orchestration).

On startup the server scans `/etc/drbd.d/`, `/etc/containers/systemd/`, and the local pcsd REST API to pre-populate the SQLite DB with node/network data.

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

### UI Workflow (7 tabs)

1. **노드 풀** (`/nodes/`) — cluster nodes, network pool, Ansible profiles (shared across all tabs)
2. **DRBD 설정** (`/drbd/`) → `.res` resource file + Ansible playbook
3. **Volume 설정** (`/volume/`) → named volumes / DRBD-backed host paths
4. **Pod 설정** (`/quadlet/`) → systemd `.container`/`.network`/`.pod`/`.volume` unit files
5. **컨테이너 설정** (`/container/`) → form-based container definition (image, volumes, env, args)
6. **방화벽** (`/nft/`) → nftables `netdev ingress` filter config
7. **Pacemaker 설정** (`/pacemaker/`) → `pcs` bash script + Ansible playbook

### Code Layout

Each subsystem has a parallel structure:

| Layer | Purpose |
|-------|---------|
| `src/models/{drbd,quadlet,pacemaker,nft}.rs` | Data structures deserialized from HTML form POST bodies |
| `src/generators/{drbd,quadlet,pacemaker,nft}.rs` | Pure functions: model → file content strings |
| `src/routes/{drbd,quadlet,pacemaker,nft,volume,container,nodes,main}.rs` | Axum handlers |
| `templates/{drbd,quadlet,pacemaker,nft,volume,container,nodes}/` | Tera templates (Jinja2 syntax) |

Supporting modules:

| File | Purpose |
|------|---------|
| `src/db.rs` | SQLite schema, CRUD for nodes/networks/ansible_profiles/node_interfaces/volumes/nft_* |
| `src/scan.rs` | Startup scan: DRBD .res, Quadlet .network/.pod files, pcsd REST API, interface auto-match |
| `src/nft_scanner.rs` | Parse existing nftables files and sync to DB (optional Ansible fetch) |
| `src/main.rs` | Router wiring, config loading, Tera init, background scan spawn |

### Key Design Points

- **No session state** — all form state is in browser localStorage. Each POST is stateless.
- **SQLite DB** (`icm.db`) stores nodes, networks, ansible_profiles, node_interfaces, volumes, and nft data across restarts.
- **Startup scan** runs in a `tokio::spawn` background task; it does not block server startup.
- **Global Ansible profile**: `ICM_PROFILE_KEY` localStorage key stores the active profile name selected in the Node Pool page. All other tabs read this key and show a profile dropdown pre-selected to the active profile. Profile selectors toggle manual SSH user/key inputs vs. hidden inputs populated from the selected profile.
- **Interface auto-detection**: `POST /api/collect-interfaces` runs an Ansible playbook that calls `ip -j addr show` on each cluster node, stores results in `node_interfaces`, then calls `auto_match_network_interfaces()` to update `networks.interface` by subnet matching.
- **`__PARENT_IFACE__` placeholder**: when a network's interface is unknown at Quadlet generation time, `generate_network_unit()` emits `Options=parent=__PARENT_IFACE__`. The generated Ansible playbook replaces this with `{{ _piface_<name>.stdout | trim }}` and inserts an `ip route show to match <subnet>` detection task.
- **Quadlet "from-inspect" path**: `POST /quadlet/from-inspect` accepts raw `podman inspect` JSON and calls `podman_inspect_to_quadlet()` without manual form entry.
- **Pod file scanning**: `scan_quadlet_pods()` in `scan.rs` reads `/etc/containers/systemd/*.pod` files, parsing `Network=name.network:ip=X:ip6=Y` entries. Results served at `/api/quadlet-pods`.
- **Pod IP addressing**: `PodNetworkEntry` has `ip: Option<String>` (IPv4, required) and `ip6: Option<String>` (IPv6, optional). Generated `.pod` file emits `ip=`, `ip6=`, or both.
- **Default Pacemaker constraints**: `generate_default_constraints()` auto-creates order and colocation constraints so Quadlet services only start after DRBD is promoted on the same node.
- **IPv4/IPv6 dual-stack**: `.network` file scanner collects all `Subnet=`/`Gateway=` lines and classifies by `:` presence into separate `subnet`/`subnet6`/`gateway`/`gateway6` fields.
- **pcsd resource sync**: `POST /api/pacemaker/pcsd-fetch` accepts credentials, logs in to the pcsd REST API, fetches cluster status (nodes, resources, constraints) and returns structured JSON. The Pacemaker page has a "pcsd 현재 설정 가져오기" card for this.
- **nftables ingress filter**: generates `table netdev` with target IP sets (from scanned pod IPs), subnet group sets, service port sets, and per-target allow/drop rules. Multiple global port rules (tcp/udp with port ranges) supported via `NftGlobalRule`.
- **Container form UI**: `/container/` uses a form-based UI (not JSON editor). Volume mounts support both registered volumes (from DB) and podman named volumes, with mount options (z, Z, ro).
- Templates are loaded at startup from `templates/**/*.html` (not hot-reloaded in release builds).
- Custom Tera filters `starts_with` and `ends_with` registered in `main.rs`.
- `become` is a Rust reserved keyword — DB/API structs use `do_become` with `#[serde(rename = "become")]`.

### HTTP Routes

```
GET  /                          → Home/overview
GET  /nodes/                    → Node pool management page
POST /nodes/scan                → Trigger DRBD/Quadlet rescan

GET  /api/nodes                 → List nodes (JSON)
POST /api/nodes                 → Add/update node
DELETE /api/nodes/:id           → Delete node

GET  /api/networks              → List networks (JSON)
POST /api/networks              → Add/update network (interface always auto-cleared)
DELETE /api/networks/:id        → Delete network

GET  /api/ansible-profiles      → List Ansible profiles (JSON)
POST /api/ansible-profiles      → Add/update profile
DELETE /api/ansible-profiles/:id → Delete profile

GET  /api/node-interfaces       → List collected node interface data (JSON)
POST /api/collect-interfaces    → Run Ansible to collect interface/IP info from nodes

GET  /api/quadlet-pods          → List scanned .pod files with network IP assignments

GET  /drbd/                     → DRBD form
POST /drbd/generate             → Generate DRBD .res file + Ansible
POST /drbd/download             → Download .res file
POST /drbd/save                 → Save resource to DB
DELETE /drbd/delete/:name       → Delete saved resource
GET  /api/lvscan                → Scan LVM volumes on local system

GET  /quadlet/                  → Quadlet (Pod 설정) form
POST /quadlet/generate          → Generate Quadlet unit files
POST /quadlet/from-inspect      → Parse podman inspect JSON → Quadlet units

GET  /volume/                   → Volume management page
POST /volume/generate           → Generate volume unit files
GET  /api/volumes               → List volumes (JSON)
POST /api/volumes               → Add/update volume
DELETE /api/volumes/:id         → Delete volume
GET  /api/drbd/resources        → List scanned DRBD resources (for volume labels)

GET  /container/                → Container settings page (form-based)
POST /container/generate        → Generate container unit files
POST /container/from-inspect    → Parse podman inspect JSON → container config

GET  /pacemaker/                → Pacemaker form (includes pcsd sync card)
POST /pacemaker/generate        → Generate pcs script + Ansible
POST /api/pacemaker/pcsd-fetch  → Fetch cluster resources/constraints from pcsd REST API

GET  /nft/                      → Firewall (nftables) form
POST /nft/generate              → Generate nftables netdev ingress config

GET  /api/nft/subnet-groups     → List subnet groups
POST /api/nft/subnet-groups     → Add/update subnet group
DELETE /api/nft/subnet-groups/:id
GET  /api/nft/services          → List service definitions
POST /api/nft/services          → Add/update service
DELETE /api/nft/services/:id
GET  /api/nft/targets           → List nft targets
POST /api/nft/targets           → Add/update target
DELETE /api/nft/targets/:id
GET  /api/nft/config            → Get global nft config
POST /api/nft/config            → Update global nft config
POST /api/nft/scan              → Scan existing nftables file and sync to DB
```

### SQLite Schema

```sql
nodes            (id, hostname, ip, ssh_user, source, created_at)
networks         (id, name, driver, interface, subnet, gateway,
                  subnet6, gateway6, ipv6, ipvlan_mode, source, created_at)
ansible_profiles (id, name, auth_method, ssh_user, ssh_key, ssh_password,
                  do_become, become_method, become_password, created_at)
node_interfaces  (id, hostname, interface, ip, prefix_len, family, created_at)
drbd_resources   (id, name, content, created_at)
volumes          (id, name, host_path, description, drbd_resource, created_at)

nft_subnet_groups (id, name, cidrs_v4_json, cidrs_v6_json, created_at)
nft_services      (id, name, tcp_ports_json, udp_ports_json, created_at)
nft_targets       (id, name, ipv4_addrs_json, ipv6_addrs_json, created_at)
nft_target_rules  (id, target_id, service_name, subnet_group_name)
nft_global_config (id, table_name, device_name, chain_name,
                   traceroute_start, traceroute_end, nft_file, updated_at)
```

### Supporting Files

- `scripts/podman_to_quadlet.sh` — bash alternative to the web UI
- `ansible/playbooks/` — three reference playbooks shown to users in generated output
- `icm.conf` — INI configuration file read at startup

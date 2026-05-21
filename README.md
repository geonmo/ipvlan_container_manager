# IPVLAN Container Manager

A web-based configuration helper for building 3-nodes High Availability container clusters on **RHEL9 / AlmaLinux9** using **DRBD + Quadlet + Pacemaker**.

> **한국어 문서**: [README.kor.md](README.kor.md)

---

## Overview

HA Container Manager is a Rust web application that generates all the configuration files needed to run Podman containers in a Primary/Secondary HA cluster:

```
[Node 1] DRBD Primary ← Promoted → Quadlet Container (IPVLAN)
                  ↕ DRBD Sync
[Node 2] DRBD Secondary ← Standby
[Node 3] DRBD Secondary ← Standby
```

Sequential configuration steps, each accessible from the navigation bar:

| Tab | Path | Output |
|-----|------|--------|
| **Node Pool** | `/nodes/` | Cluster nodes, network pool, Ansible profiles |
| **DRBD** | `/drbd/` | `.res` resource file + Ansible playbook |
| **Volume** | `/volume/` | Named volume registry (DRBD-backed or host path) |
| **Pod** | `/quadlet/` | `.container` / `.pod` / `.volume` / `.network` systemd unit files |
| **Container** | `/container/` | Container unit files (form-based; image, volumes, env, args) |
| **Firewall** | `/nft/` | nftables `netdev ingress` filter config |
| **Pacemaker** | `/pacemaker/` | `pcs` bash script + Ansible playbook |
| **Cluster Status** | `/cluster/` | Live topology viewer — resources, constraints, node state |

On startup the server automatically scans `/etc/drbd.d/`, `/etc/containers/systemd/`, and the local pcsd REST API to pre-populate the node pool, network pool, and cluster state.

---

## Prerequisites

All nodes must run **AlmaLinux9 or RHEL9**.

**Required packages (each cluster node):**

```bash
# Enable ELRepo
dnf install -y elrepo-release
dnf config-manager --enable elrepo

# DRBD
dnf install -y drbd9x-utils kmod-drbd9x

# Pacemaker / Corosync
dnf install -y pacemaker pcs corosync

# Podman
dnf install -y podman
```

**Management host (where this app runs):**

```bash
pip install ansible
```

**Firewall rules (cluster nodes):**

```bash
firewall-cmd --permanent --add-port=7789/tcp   # DRBD replication
firewall-cmd --permanent --add-port=5404/udp   # Corosync
firewall-cmd --permanent --add-port=5405/udp
firewall-cmd --reload
```

**Network:**
- A dedicated sync NIC between nodes is recommended for DRBD replication
- Configure the host NIC for IPVLAN container networking

---

## Configuration File

`icm.conf` is read from the working directory at startup. Missing keys fall back to defaults.

```ini
[server]
host = 0.0.0.0   # 127.0.0.1 for localhost-only
port = 5000

[logging]
level = info      # error | warn | info | debug | trace

[database]
path = ./icm.db

[paths]
drbd_dir    = /etc/drbd.d
quadlet_dir = /etc/containers/systemd
temp_dir    = /tmp/icm
deploy_mode = test   # "test" writes to temp_dir

[pacemaker]
pcsd_url      = https://localhost:2224
pcsd_user     = hacluster
pcsd_password =
```

`RUST_LOG` environment variable takes precedence over `[logging]`.

---

## Running

```bash
git clone <repo-url>
cd ipvlan_container_manager
cargo build --release

# Edit configuration if needed
vi icm.conf

cargo run --release

# Override log level at runtime
RUST_LOG=debug cargo run
```

Open `http://<host>:<port>` (default: `http://localhost:5000`).

---

## Usage

### Node Pool (`/nodes/`)

Manages cluster nodes, network definitions, and Ansible deployment profiles. Data is shared across all tabs via browser localStorage.

#### Ansible Profiles

Profiles store SSH connection parameters for server-side Ansible runs.

| Field | Description |
|-------|-------------|
| Profile name | Identifier used to select this profile |
| Auth method | `key` (SSH private key path) or `password` |
| SSH user | Remote user (default: `root`) |
| SSH key / password | Path to private key or password |
| become | Enable `ansible_become`; method and optional become password |

Set one profile as the **active profile** — all other tabs pre-select it automatically.

> **Note**: Passwords are stored in plaintext in the local SQLite database. Use only in test/lab environments.

#### Cluster Nodes

Add nodes manually or click **Rescan** to re-read `/etc/drbd.d/` and the pcsd API. Scanned nodes are tagged `scanned`; manual ones are tagged `manual`.

#### Network Pool

Defines IPVLAN/macvlan networks for containers. The host interface is filled in automatically by **Interface Auto-Detection**:

1. Click **인터페이스 자동감지** in the Network Pool card header
2. Select an Ansible profile
3. Click **수집 시작** — the server runs an Ansible playbook that executes `ip -j addr show` on every node, saves the result, and matches each network's subnet to the correct interface

If no match is found at Quadlet generation time, a `__PARENT_IFACE__` placeholder is emitted and the Ansible playbook includes an `ip route show` task to resolve it at deploy time.

---

### Step 1 — DRBD (`/drbd/`)

| Field | Description | Example |
|-------|-------------|---------|
| Resource Name | Name of the DRBD resource (`<name>.res`) | `r0` |
| Protocol | Sync mode — Protocol C recommended | `C` |
| Minor Number | Device path `/dev/drbd<minor>` | `0` |
| Node Hostname | FQDN or hostname | `node1.ha.local` |
| Node IP | IP used for DRBD replication | `192.168.10.11` |
| Disk type | `block` (raw device) or `lvm` | `block` |
| Port | DRBD listen port | `7789` |

Supports 2–7 nodes. Advanced options (Net / Disk / Startup) are under the collapsible panel.

**Output:** DRBD `.res` file, initialization commands, Ansible playbook.

---

### Step 2 — Volume (`/volume/`)

Register named volumes referenced by Pod and Container tabs.

| Field | Description |
|-------|-------------|
| Name | Quadlet `.volume` filename stem |
| Host path | Bind-mount path (e.g., `/mnt/drbd/data`) |
| Description | Optional documentation |
| DRBD resource | Links this volume to a DRBD resource |

Registered volumes appear in the Container tab's volume mount dropdown.

---

### Step 3 — Pod / Quadlet (`/quadlet/`)

#### Option A: Manual Entry

Fill in network, volume, pod, and container sections. Pod network entries require an **IPv4 address** (mandatory) and optionally an **IPv6 address**.

#### Option B: From `podman inspect`

Paste the output of `podman inspect <container>` — the app parses metadata and generates Quadlet unit files.

**Output files:**
- `<name>.network` — IPVLAN/macvlan network (dual-stack IPv4+IPv6)
- `<name>.volume` — Named volume
- `<name>.pod` — Pod grouping unit
- `<name>.container` — Container service unit

Deploy to `/etc/containers/systemd/` on both nodes, then `systemctl daemon-reload`.

#### CLI Alternative

```bash
./scripts/podman_to_quadlet.sh --network ipvlan0 myapp mydb
./scripts/podman_to_quadlet.sh --install --network ipvlan0 myapp
```

Requires: `podman`, `jq`

---

### Step 4 — Container (`/container/`)

Form-based container definition:

- **Container name** + **Pod** (selected from scanned pods)
- **Image** URL
- **Volume mounts**: pick from registered volumes or enter a named volume; mount options (`:z`, `:Z`, `:ro,z`, `:ro`)
- **Environment variables**: key-value pairs
- **Arguments**: extra command-line arguments

Generates `.container` unit files and an Ansible deployment playbook.

---

### Step 5 — Firewall / nftables (`/nft/`)

Generates an nftables `netdev ingress` filter for IPVLAN container interfaces.

**Global settings:**
- **Device**: host network interface (from collected interfaces dropdown)
- **Global rules**: tcp/udp + port range rules applied to all pods (e.g., allow traceroute UDP 33434–65535)

**Per-pod rules:**
- Select a pod (IPs populated automatically from pod scan data)
- Assign services (TCP/UDP ports) and optionally restrict source by subnet group

**Output** — `.nft` file with:
- Target IP sets per pod (`target_<name>_v4`, `target_<name>_v6`)
- Subnet group sets (`sg_<name>_v4`, `sg_<name>_v6`)
- Service port sets + per-target allow/drop rules

---

### Step 6 — Pacemaker (`/pacemaker/`)

The page is organized as collapsible accordion sections:

1. **Cluster basics** — cluster name, node selection, quorum policy, STONITH
2. **DRBD + Filesystem** — one card per DRBD volume (Promotable Clone + FS resource); supports multiple volumes per service
3. **Resource groups** — Pod + Container grouping (JSON)
4. **Systemd resources** — Quadlet service registrations (JSON)
5. **Constraints** — auto Order/Colocation + manual overrides
6. **Ansible deployment** — SSH profile and target hosts

**pcsd sync** (card above the form): enter the pcsd URL — the app reads the node token from `/var/lib/pcsd/known-hosts` and fetches current cluster resources and constraints. Each detected service group has a **"폼에 적용"** button to auto-populate the form. Full topology is shown in the **Cluster Status** tab.

**Auto constraints** (when checked):
- **Order**: DRBD clone must be promoted before services start
- **Colocation**: services run only on the DRBD Primary node

**Output:** `pcs` bash script, CIB XML reference, Ansible playbook.

---

### Cluster Status (`/cluster/`)

Read-only topology viewer. Enter the pcsd URL and click **클러스터 정보 가져오기**:

- Independent services are separated by Union-Find analysis of order/colocation constraints
- Each service shows a pipeline: **DRBD Clone → FS → Group → Systemd** with running node and status badges
- DRBD clones are identified by matching against DB-registered DRBD resource names (not name heuristics)

---

## Deployment

Each configuration step produces an Ansible playbook. Run from the management host:

```bash
ansible-playbook -i inventory.yml ansible/playbooks/deploy_drbd.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_quadlet.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_pacemaker.yml
```

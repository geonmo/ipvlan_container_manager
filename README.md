# HA Container Manager

A web-based configuration helper for building 2-node High Availability container clusters on **RHEL9 / AlmaLinux9** using **DRBD + Quadlet + Pacemaker**.

---

## Table of Contents

- [English](#english)
- [한국어](#한국어)

---

## English

### Overview

HA Container Manager is a Rust web application that generates all the configuration files needed to run Podman containers in an Active/Passive HA cluster:

```
[Node 1] DRBD Primary ← Promoted → Quadlet Container (IPVLAN)
                  ↕ DRBD Sync
[Node 2] DRBD Secondary ← Standby
```

The tool guides you through sequential configuration steps, all accessible from the navigation bar:

| Tab | Path | Output |
|-----|------|--------|
| **노드 풀** | `/nodes/` | Cluster nodes, network pool, Ansible profiles |
| **DRBD 설정** | `/drbd/` | `.res` resource file + Ansible playbook |
| **Volume 설정** | `/volume/` | Named volume registry (DRBD-backed or host path) |
| **Pod 설정** | `/quadlet/` | `.container` / `.pod` / `.volume` / `.network` systemd unit files |
| **컨테이너 설정** | `/container/` | Container unit files (form-based; image, volumes, env, args) |
| **방화벽** | `/nft/` | nftables `netdev ingress` filter config |
| **Pacemaker 설정** | `/pacemaker/` | `pcs` bash script + Ansible playbook |

On startup the server automatically scans `/etc/drbd.d/`, `/etc/containers/systemd/`, and the local pcsd REST API to pre-populate the node pool, network pool, and cluster state from the running system.

### Prerequisites

All nodes must run **AlmaLinux9 or RHEL9**.

**Required packages (on each cluster node):**

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

**Firewall rules (on cluster nodes):**

```bash
# DRBD replication port (default 7789)
firewall-cmd --permanent --add-port=7789/tcp

# Corosync
firewall-cmd --permanent --add-port=5404/udp
firewall-cmd --permanent --add-port=5405/udp

firewall-cmd --reload
```

**Network:**
- A dedicated sync NIC between nodes is recommended for DRBD replication
- Configure the host NIC for IPVLAN container networking

### Configuration File

The application reads `icm.conf` from the working directory on startup. If the file is not found, defaults are used.

```ini
[server]
# Bind address (0.0.0.0 = all interfaces, 127.0.0.1 = localhost only)
host = 0.0.0.0
# Listen port
port = 5000

[logging]
# Log level: error | warn | info | debug | trace
level = info

[database]
path = ./icm.db

[paths]
# Where to scan for DRBD resource files
drbd_dir = /etc/drbd.d
# Where to scan for Quadlet network/pod files
quadlet_dir = /etc/containers/systemd
# Temporary directory for Ansible inventory/playbook generation
temp_dir = /tmp/icm
# "test" uses temp_dir; future "deploy" will write directly to the system
deploy_mode = test

[pacemaker]
pcsd_url = https://localhost:2224
pcsd_user = hacluster
pcsd_password =
```

`RUST_LOG` environment variable takes precedence over the `[logging]` section when set.

### Running the Application

```bash
# Clone and build
git clone <repo-url>
cd ipvlan_container_manager
cargo build --release

# Edit configuration if needed
vi icm.conf

# Run
cargo run --release

# Override log level at runtime
RUST_LOG=debug cargo run
```

Open `http://<host>:<port>` in your browser (default: `http://localhost:5000`).

---

### Node Pool

Navigate to **노드 풀** (Node Pool) in the top menu to manage cluster nodes, network definitions, and Ansible deployment profiles. Data entered here is shared across all tabs via browser localStorage.

#### Ansible Profiles

Profiles store SSH connection parameters used by server-side Ansible runs.

| Field | Description |
|-------|-------------|
| Profile name | Identifier used to select this profile |
| Auth method | `key` (SSH private key path) or `password` |
| SSH user | Remote user (default: `root`) |
| SSH key / password | Path to private key, or the password itself |
| become | Enable `ansible_become`; method and optional become password |

**Active profile**: Select a profile as the "기본 Ansible 프로파일" — all other tabs will automatically pre-select this profile for Ansible deployment.

> **Note**: Passwords are stored in plaintext in the local SQLite database. Use only in test/lab environments.

#### Cluster Nodes

Add or remove cluster nodes manually, or use the **시스템 재스캔** (Rescan) button to re-read `/etc/drbd.d/` and the pcsd API. Nodes discovered by scan are tagged `scanned`; manually added nodes are tagged `manual`.

#### Network Pool

Defines the IPVLAN/macvlan networks that containers will attach to. The **host interface** field is left empty on entry — it is filled in automatically by the **인터페이스 자동감지** (Interface Auto-Detection) feature.

**Interface Auto-Detection** (requires at least one Ansible profile and at least one node):

1. Click **인터페이스 자동감지** in the Network Pool card header.
2. Select an Ansible profile from the dropdown.
3. Click **수집 시작** — the server runs an Ansible playbook that executes `ip -j addr show` on every node, saves the result, and then matches each network's subnet to the correct interface.

If no match is found at Quadlet generation time, a `__PARENT_IFACE__` placeholder is emitted and the generated Ansible playbook includes a `ip route show` task to resolve it at deploy time.

---

### Step 1 — DRBD Configuration

Navigate to **DRBD** in the top menu.

| Field | Description | Example |
|-------|-------------|---------|
| Resource Name | Name of the DRBD resource (becomes `<name>.res`) | `r0` |
| Protocol | Sync mode — Protocol C recommended for HA | `C` |
| Minor Number | Determines device path `/dev/drbd<minor>` | `0` |
| Node Hostname | FQDN or hostname of each node | `node1.ha.local` |
| Node IP | IP address used for DRBD replication | `192.168.10.11` |
| Disk type | `block` (raw device) or `lvm` (logical volume) | `block` |
| Port | DRBD listen port (default 7789) | `7789` |

Supports 2 to 7 nodes. Advanced options (Net / Disk / Startup) are available under the collapsible panel.

**Output:**
- DRBD `.res` file (copy to `/etc/drbd.d/` on both nodes)
- DRBD initialization commands
- Ready-to-run Ansible playbook

---

### Step 2 — Volume Settings

Navigate to **Volume** in the top menu. Register named volumes that will be referenced by the Pod and Container configuration steps.

Each volume has:
- **Name**: used as the Quadlet `.volume` filename stem
- **Host path**: the bind-mount path on the host (e.g., `/mnt/drbd/data`)
- **Description** (optional): for documentation
- **DRBD resource** (optional): links this volume to a DRBD resource for labeling in the Container settings UI

Volumes registered here appear in the Container settings tab's volume mount dropdown.

---

### Step 3 — Pod Settings (Quadlet)

Navigate to **Pod** in the top menu.

#### Option A: Manual Entry

Fill in the network, volume, pod, and container sections. Pod network entries require an **IPv4 address** (mandatory) and optionally an **IPv6 address**.

#### Option B: From `podman inspect`

Switch to the **podman inspect JSON** tab and paste the output of `podman inspect <container_name>`. The application parses the running container metadata and generates corresponding Quadlet unit files.

**Output files:**
- `<name>.network` — IPVLAN/macvlan network definition (dual-stack IPv4+IPv6 supported)
- `<name>.volume` — Named volume
- `<name>.pod` — Pod grouping unit
- `<name>.container` — Container service unit

Deploy to `/etc/containers/systemd/` on both nodes, then `systemctl daemon-reload`.

#### CLI Alternative

```bash
./scripts/podman_to_quadlet.sh --network ipvlan0 myapp mydb
./scripts/podman_to_quadlet.sh --install --network ipvlan0 myapp
```

Requirements: `podman`, `jq`

---

### Step 4 — Container Settings

Navigate to **컨테이너** in the top menu.

Form-based container definition (no JSON editing required):

- **Container name** and **Pod** (selected from scanned pods or Node Pool data)
- **Image**: container image URL
- **Volume mounts**: pick from registered volumes or enter a named volume; choose mount options (`:z`, `:Z`, `:ro,z`, `:ro`)
- **Environment variables**: key-value pairs
- **Arguments**: extra command-line arguments

Generates `.container` unit files and an Ansible deployment playbook.

---

### Step 5 — Firewall (nftables)

Navigate to **방화벽** in the top menu.

Generates an nftables `netdev ingress` filter for IPVLAN container interfaces.

**Global settings:**
- **Device**: select the host network interface from collected interfaces
- **Global rules**: define multiple protocol (tcp/udp) + port range rules that apply to all pods (e.g., allow traceroute UDP 33434–65535)

**Per-pod rules:**
- Select a pod from the dropdown (IPs are populated automatically from pod scan data)
- Assign services (with TCP/UDP ports) and optionally restrict source by subnet group

**Output**: a `.nft` configuration file loaded by nftables, with:
- Target IP sets per pod (`target_<name>_v4`, `target_<name>_v6`)
- Subnet group sets (`sg_<name>_v4`, `sg_<name>_v6`)
- Service port sets
- Per-target allow rules + drop-with-log rules

---

### Step 6 — Pacemaker Configuration

Navigate to **Pacemaker** in the top menu.

#### pcsd Sync

Use the **pcsd 현재 설정 가져오기** card at the top of the page to fetch the current cluster configuration directly from pcsd (port 2224):

1. Enter pcsd URL, username, and password
2. Click **가져오기**
3. The current nodes, resources, and constraints are displayed
4. Click **노드 적용** to populate the node selection from pcsd data

#### Form Fields

| Field | Description |
|-------|-------------|
| Cluster Name | Pacemaker cluster name |
| Nodes | Select from Node Pool |
| DRBD Resource | Pacemaker resource ID for DRBD promotable clone |
| Systemd Resources | Quadlet services to register (JSON array) |
| Auto Constraints | Auto-generate Order + Colocation constraints |

**Auto constraints** (generated when checked):
- **Order**: DRBD clone must be promoted before services start
- **Colocation**: services run only on the DRBD Primary node

**Output:**
- `pcs` bash script
- CIB XML reference snippet
- Ansible playbook for automated deployment

---

### Deployment

Each step produces an Ansible playbook and inventory. Run them from the management host:

```bash
# Step 1: Deploy DRBD
ansible-playbook -i inventory.yml ansible/playbooks/deploy_drbd.yml

# Step 2: Deploy Quadlet units
ansible-playbook -i inventory.yml ansible/playbooks/deploy_quadlet.yml

# Step 3: Configure Pacemaker
ansible-playbook -i inventory.yml ansible/playbooks/deploy_pacemaker.yml
```

---

## 한국어

### 개요

HA Container Manager는 **RHEL9 / AlmaLinux9** 환경에서 **DRBD + Quadlet + Pacemaker**를 사용하는 2-노드 Active/Passive HA 컨테이너 클러스터를 구성하는 데 필요한 모든 설정 파일을 생성해주는 Rust 기반 웹 애플리케이션입니다.

```
[노드 1] DRBD Primary ← Promoted → Quadlet 컨테이너 (IPVLAN)
                   ↕ DRBD 동기화
[노드 2] DRBD Secondary ← 대기
```

내비게이션 바의 탭으로 단계별 설정을 진행합니다:

| 탭 | 경로 | 출력물 |
|----|------|--------|
| **노드 풀** | `/nodes/` | 클러스터 노드, 네트워크 풀, Ansible 프로파일 |
| **DRBD 설정** | `/drbd/` | `.res` 리소스 파일 + Ansible 플레이북 |
| **Volume 설정** | `/volume/` | Named 볼륨 레지스트리 (DRBD 연동 또는 호스트 경로) |
| **Pod 설정** | `/quadlet/` | `.container` / `.pod` / `.volume` / `.network` systemd 유닛 파일 |
| **컨테이너 설정** | `/container/` | 컨테이너 유닛 파일 (폼 기반; 이미지, 볼륨, env, args) |
| **방화벽** | `/nft/` | nftables `netdev ingress` 필터 설정 |
| **Pacemaker 설정** | `/pacemaker/` | `pcs` 배시 스크립트 + Ansible 플레이북 |

서버 시작 시 `/etc/drbd.d/`, `/etc/containers/systemd/`, 로컬 pcsd REST API를 자동으로 스캔하여 노드 풀, 네트워크 풀, 클러스터 상태를 사전에 채웁니다.

### 사전 요구사항

모든 노드는 **AlmaLinux9 또는 RHEL9**이어야 합니다.

**각 클러스터 노드에 설치할 패키지:**

```bash
dnf install -y elrepo-release
dnf config-manager --enable elrepo
dnf install -y drbd9x-utils kmod-drbd9x
dnf install -y pacemaker pcs corosync
dnf install -y podman
```

**관리 서버 (이 앱이 실행되는 서버):**

```bash
pip install ansible
```

**클러스터 노드 방화벽 설정:**

```bash
firewall-cmd --permanent --add-port=7789/tcp
firewall-cmd --permanent --add-port=5404/udp
firewall-cmd --permanent --add-port=5405/udp
firewall-cmd --reload
```

### 설정 파일

```ini
[server]
host = 0.0.0.0
port = 5000

[logging]
level = info

[database]
path = ./icm.db

[paths]
drbd_dir = /etc/drbd.d
quadlet_dir = /etc/containers/systemd
temp_dir = /tmp/icm
deploy_mode = test

[pacemaker]
pcsd_url = https://localhost:2224
pcsd_user = hacluster
pcsd_password =
```

### 애플리케이션 실행

```bash
git clone <repo-url>
cd ipvlan_container_manager
cargo build --release
vi icm.conf
cargo run --release
```

브라우저에서 `http://<host>:<port>` 접속 (기본값: `http://localhost:5000`).

---

### 노드 풀 관리

상단 메뉴에서 **노드 풀**을 클릭합니다. 여기서 정의한 정보는 모든 탭에서 공유됩니다.

#### Ansible 프로파일

서버 측 Ansible 실행(인터페이스 수집, 배포)에 사용할 SSH 접속 정보를 저장합니다.

**기본 Ansible 프로파일**: 노드 풀에서 프로파일을 선택해두면 다른 모든 탭에서 해당 프로파일이 자동으로 선택됩니다.

> **주의**: 비밀번호는 로컬 SQLite DB에 평문으로 저장됩니다. 테스트/랩 환경에서만 사용하세요.

#### 클러스터 노드

수동 추가/삭제 또는 **시스템 재스캔** 버튼으로 갱신할 수 있습니다.

#### 네트워크 풀

컨테이너가 연결할 IPVLAN/macvlan 네트워크를 정의합니다. 호스트 인터페이스는 **인터페이스 자동감지** 기능으로 자동 매핑됩니다.

---

### 1단계 — DRBD 설정

`.res` 파일, DRBD 초기화 명령어, 전체 Ansible 플레이북을 생성합니다.

---

### 2단계 — Volume 설정

Pod/컨테이너에서 사용할 볼륨을 등록합니다. DRBD 리소스 연동 정보도 입력할 수 있으며, 컨테이너 설정 탭에서 드롭다운으로 선택할 수 있습니다.

---

### 3단계 — Pod 설정 (Quadlet)

Pod 네트워크 항목에는 **IPv4 주소 (필수)** 와 **IPv6 주소 (선택)** 를 입력합니다.

생성 파일: `*.network`, `*.volume`, `*.pod`, `*.container`

양쪽 노드의 `/etc/containers/systemd/`에 배포 후 `systemctl daemon-reload` 실행.

---

### 4단계 — 컨테이너 설정

폼 기반 UI로 컨테이너를 구성합니다:

- 컨테이너 이름 + Pod 선택 (스캔된 pod 드롭다운)
- 컨테이너 이미지 주소
- 볼륨 마운트: 등록된 볼륨 또는 named 볼륨 선택, 마운트 옵션 (`:z`, `:Z`, `:ro,z`, `:ro`)
- 환경변수 (키-값)
- 추가 인자

---

### 5단계 — 방화벽 (nftables)

IPVLAN 컨테이너 인터페이스용 nftables `netdev ingress` 필터를 생성합니다.

- **전역 설정**: 인터페이스 선택 (수집된 인터페이스 드롭다운) + 여러 개의 전역 허용 규칙 (프로토콜 + 포트 범위)
- **Pod별 규칙**: Pod를 드롭다운으로 선택하면 IP 주소가 자동 입력됨. 서비스(포트)와 소스 서브넷 그룹을 설정

---

### 6단계 — Pacemaker 설정

**pcsd 현재 설정 가져오기** 카드에서 pcsd URL, 사용자명, 비밀번호를 입력하고 **가져오기** 버튼을 클릭하면 현재 클러스터 노드, 리소스, 제약조건 정보가 표시되고 폼에 자동 반영됩니다.

**출력물**: `pcs` 배시 스크립트, CIB XML 스니펫, Ansible 플레이북

---

### 배포

```bash
ansible-playbook -i inventory.yml ansible/playbooks/deploy_drbd.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_quadlet.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_pacemaker.yml
```

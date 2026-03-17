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

The tool guides you through three sequential steps:

| Step | Tool | Output |
|------|------|--------|
| 1 | **DRBD** | `.res` resource file + Ansible inventory |
| 2 | **Quadlet** | `.container` / `.pod` / `.volume` / `.network` systemd unit files |
| 3 | **Pacemaker** | `pcs` bash script + Ansible playbooks |

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

### Step 1 — DRBD Configuration

Navigate to **DRBD** in the top menu.

**Fill in the form:**

| Field | Description | Example |
|-------|-------------|---------|
| Resource Name | Name of the DRBD resource (becomes `<name>.res`) | `r0` |
| Protocol | Sync mode — Protocol C recommended for HA | `C` |
| Minor Number | Determines device path `/dev/drbd<minor>` | `0` |
| Node 1/2 Hostname | FQDN or hostname of each node | `node1.ha.local` |
| Node 1/2 IP | IP address used for DRBD replication | `192.168.10.11` |
| Node 1/2 Disk | Block device to replicate | `/dev/sdb` |
| Port | DRBD listen port (default 7789) | `7789` |

Advanced options (Net / Disk / Startup) are available under the collapsible panel.

**Output:**
- DRBD `.res` file content (copy to `/etc/drbd.d/` on both nodes)
- Ansible inventory YAML
- DRBD initialization commands (`drbdadm create-md`, `drbdadm up`, etc.)
- Ready-to-run Ansible playbook

Click **Download .res** to save the file directly.

### Step 2 — Quadlet Unit Generation

Navigate to **Quadlet** in the top menu.

#### Option A: Manual Entry

Fill in the network, volume, pod, and container sections. The container section accepts a JSON array — click **예시 (Example)** to see the format.

#### Option B: From `podman inspect`

Switch to the **podman inspect JSON** tab and paste the output of:

```bash
podman inspect <container_name>
```

The application parses the running container metadata and generates the corresponding Quadlet unit files automatically.

**Output files:**
- `<name>.network` — IPVLAN/macvlan network definition
- `<name>.volume` — Named volume
- `<name>.pod` — Pod grouping
- `<name>.container` — Container service unit

Deploy to `/etc/containers/systemd/` on both nodes, then `systemctl daemon-reload`.

#### CLI Alternative

A bash helper script is also provided:

```bash
# Convert all running containers
./scripts/podman_to_quadlet.sh

# Convert specific containers with IPVLAN network
./scripts/podman_to_quadlet.sh --network ipvlan0 myapp mydb

# Convert and install directly to /etc/containers/systemd/
./scripts/podman_to_quadlet.sh --install --network ipvlan0 myapp

# Options:
#   -o, --output-dir DIR   Output directory (default: ./quadlet-units)
#   -i, --install          Install to /etc/containers/systemd/
#   -n, --network NAME     IPVLAN network name
#   -d, --drbd RESOURCE    DRBD resource name (added as a comment)
```

Requirements: `podman`, `jq`

### Step 3 — Pacemaker Configuration

Navigate to **Pacemaker** in the top menu.

**Fill in the form:**

| Field | Description | Example |
|-------|-------------|---------|
| Cluster Name | Name of the Pacemaker cluster | `ha-cluster` |
| Node 1/2 Name | Node hostnames (must match DRBD nodes) | `node1.ha.local` |
| DRBD Resource Name | Pacemaker resource ID for DRBD | `drbd-r0` |
| DRBD .res Name | Resource name from the `.res` file | `r0` |
| Clone Name | Promotable clone ID | `drbd-r0-clone` |
| Systemd Resources | JSON array of Quadlet services to register | see below |
| Auto Constraints | Auto-generate Order + Colocation constraints | checked |

**Systemd resources JSON format:**

```json
[
  {
    "resource_name": "svc-myapp",
    "systemd_unit": "myapp.service",
    "resource_type": "container",
    "monitor_interval": "30s",
    "start_timeout": "60s",
    "stop_timeout": "60s",
    "clone": false
  }
]
```

**Auto constraints** (generated when checked):
- **Order**: DRBD clone must be `promote`d before services `start`
- **Colocation**: Services run only on the DRBD Promoted (Primary) node

**Output:**
- `pcs` bash script to configure the cluster
- CIB XML reference snippet
- Ansible playbook for automated deployment

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

세 단계로 구성됩니다:

| 단계 | 도구 | 출력물 |
|------|------|--------|
| 1 | **DRBD** | `.res` 리소스 파일 + Ansible 인벤토리 |
| 2 | **Quadlet** | `.container` / `.pod` / `.volume` / `.network` systemd 유닛 파일 |
| 3 | **Pacemaker** | `pcs` 배시 스크립트 + Ansible 플레이북 |

### 사전 요구사항

모든 노드는 **AlmaLinux9 또는 RHEL9**이어야 합니다.

**각 클러스터 노드에 설치할 패키지:**

```bash
# ELRepo 저장소 활성화
dnf install -y elrepo-release
dnf config-manager --enable elrepo

# DRBD
dnf install -y drbd9x-utils kmod-drbd9x

# Pacemaker / Corosync
dnf install -y pacemaker pcs corosync

# Podman
dnf install -y podman
```

**관리 서버 (이 앱이 실행되는 서버):**

```bash
pip install ansible
```

**클러스터 노드 방화벽 설정:**

```bash
# DRBD 복제 포트 (기본 7789)
firewall-cmd --permanent --add-port=7789/tcp

# Corosync
firewall-cmd --permanent --add-port=5404/udp
firewall-cmd --permanent --add-port=5405/udp

firewall-cmd --reload
```

**네트워크:**
- DRBD 복제용 전용 NIC 구성 권장
- IPVLAN 컨테이너 네트워크를 위한 호스트 NIC 설정 필요

### 설정 파일

애플리케이션 시작 시 실행 디렉토리의 `icm.conf` 파일을 읽습니다. 파일이 없으면 기본값을 사용합니다.

```ini
[server]
# 바인딩 주소 (0.0.0.0 = 모든 인터페이스, 127.0.0.1 = 로컬호스트만)
host = 0.0.0.0
# 수신 포트
port = 5000

[logging]
# 로그 레벨: error | warn | info | debug | trace
level = info
```

`RUST_LOG` 환경변수가 설정되어 있으면 `[logging]` 섹션보다 우선 적용됩니다.

### 애플리케이션 실행

```bash
# 클론 및 빌드
git clone <repo-url>
cd ipvlan_container_manager
cargo build --release

# 필요시 설정 파일 수정
vi icm.conf

# 실행
cargo run --release

# 로그 레벨 런타임 오버라이드
RUST_LOG=debug cargo run
```

브라우저에서 `http://<host>:<port>` 접속 (기본값: `http://localhost:5000`).

### 1단계 — DRBD 설정

상단 메뉴에서 **DRBD**를 클릭합니다.

**폼 입력 항목:**

| 항목 | 설명 | 예시 |
|------|------|------|
| 리소스 이름 | DRBD 리소스 이름 (`<name>.res` 파일 생성) | `r0` |
| 프로토콜 | 동기 방식 — HA 환경에서는 Protocol C 권장 | `C` |
| Minor 번호 | 장치 경로 `/dev/drbd<minor>` 결정 | `0` |
| 노드 1/2 호스트명 | 각 노드의 FQDN 또는 호스트명 | `node1.ha.local` |
| 노드 1/2 IP | DRBD 복제에 사용할 IP 주소 | `192.168.10.11` |
| 노드 1/2 디스크 | 복제할 블록 디바이스 | `/dev/sdb` |
| 포트 | DRBD 수신 포트 (기본 7789) | `7789` |

고급 옵션(Net / Disk / Startup)은 접이식 패널에서 설정할 수 있습니다.

**출력물:**
- DRBD `.res` 파일 내용 (양쪽 노드의 `/etc/drbd.d/`에 복사)
- Ansible 인벤토리 YAML
- DRBD 초기화 명령어 (`drbdadm create-md`, `drbdadm up` 등)
- 바로 실행 가능한 Ansible 플레이북

**`.res 다운로드`** 버튼으로 파일을 직접 저장할 수 있습니다.

### 2단계 — Quadlet 유닛 파일 생성

상단 메뉴에서 **Quadlet**을 클릭합니다.

#### 방법 A: 수동 입력

네트워크, 볼륨, Pod, 컨테이너 섹션을 직접 입력합니다. 컨테이너 섹션은 JSON 배열 형식이며, **예시** 버튼을 클릭하면 형식을 확인할 수 있습니다.

#### 방법 B: `podman inspect` JSON 사용

**podman inspect JSON** 탭으로 전환 후 아래 명령 출력을 붙여넣습니다:

```bash
podman inspect <컨테이너_이름>
```

실행 중인 컨테이너의 메타데이터를 자동으로 분석하여 Quadlet 유닛 파일을 생성합니다.

**생성되는 파일:**
- `<name>.network` — IPVLAN/macvlan 네트워크 정의
- `<name>.volume` — Named 볼륨
- `<name>.pod` — Pod 그룹
- `<name>.container` — 컨테이너 서비스 유닛

양쪽 노드의 `/etc/containers/systemd/`에 배포 후 `systemctl daemon-reload` 실행.

#### CLI 대안 (bash 스크립트)

```bash
# 실행 중인 모든 컨테이너 변환
./scripts/podman_to_quadlet.sh

# IPVLAN 네트워크를 지정하여 특정 컨테이너 변환
./scripts/podman_to_quadlet.sh --network ipvlan0 myapp mydb

# 변환 후 /etc/containers/systemd/ 에 직접 설치
./scripts/podman_to_quadlet.sh --install --network ipvlan0 myapp

# 옵션:
#   -o, --output-dir DIR   출력 디렉토리 (기본: ./quadlet-units)
#   -i, --install          /etc/containers/systemd/ 에 직접 설치
#   -n, --network NAME     IPVLAN 네트워크 이름 지정
#   -d, --drbd RESOURCE    DRBD 리소스 이름 (주석으로 추가)
```

요구사항: `podman`, `jq`

### 3단계 — Pacemaker 설정

상단 메뉴에서 **Pacemaker**를 클릭합니다.

**폼 입력 항목:**

| 항목 | 설명 | 예시 |
|------|------|------|
| 클러스터 이름 | Pacemaker 클러스터 이름 | `ha-cluster` |
| 노드 1/2 이름 | 노드 호스트명 (DRBD 노드와 일치해야 함) | `node1.ha.local` |
| DRBD 리소스 이름 | DRBD용 Pacemaker 리소스 ID | `drbd-r0` |
| DRBD .res 이름 | `.res` 파일의 리소스 이름 | `r0` |
| Clone 이름 | Promotable 클론 ID | `drbd-r0-clone` |
| Systemd 리소스 | 등록할 Quadlet 서비스 JSON 배열 | 아래 참조 |
| 자동 제약조건 | Order + Colocation 제약조건 자동 생성 | 체크됨 |

**Systemd 리소스 JSON 형식:**

```json
[
  {
    "resource_name": "svc-myapp",
    "systemd_unit": "myapp.service",
    "resource_type": "container",
    "monitor_interval": "30s",
    "start_timeout": "60s",
    "stop_timeout": "60s",
    "clone": false
  }
]
```

**자동 제약조건** (체크 시 자동 생성):
- **Order**: DRBD 클론이 `promote` 된 후에만 서비스 `start`
- **Colocation**: DRBD Promoted(Primary) 노드에서만 서비스 실행 (INFINITY)

**출력물:**
- 클러스터 구성용 `pcs` 배시 스크립트
- CIB XML 참조 스니펫
- 자동 배포용 Ansible 플레이북

### 배포

각 단계에서 Ansible 플레이북과 인벤토리가 생성됩니다. 관리 서버에서 실행:

```bash
# 1단계: DRBD 배포
ansible-playbook -i inventory.yml ansible/playbooks/deploy_drbd.yml

# 2단계: Quadlet 유닛 배포
ansible-playbook -i inventory.yml ansible/playbooks/deploy_quadlet.yml

# 3단계: Pacemaker 설정
ansible-playbook -i inventory.yml ansible/playbooks/deploy_pacemaker.yml
```

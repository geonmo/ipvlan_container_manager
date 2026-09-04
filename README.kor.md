# HA Container Manager (한국어)

**RHEL9 / AlmaLinux9**에서 **DRBD + Quadlet + Pacemaker**를 사용하는 2-노드 HA 컨테이너 클러스터 설정 파일을 생성해주는 웹 도우미입니다.

> **English documentation**: [README.md](README.md)
> **더 알아보기**: 코드 구조/함수 레퍼런스는 [DEVEL.md](DEVEL.md), 기능 개선 계획 및 검증 방법은 [PLAN.md](PLAN.md)

---

## 개요

HA Container Manager는 Podman 컨테이너를 Active/Passive HA 클러스터로 운용하는 데 필요한 모든 설정 파일을 생성하는 Rust 기반 웹 애플리케이션입니다.

```
[노드 1] DRBD Primary ← Promoted → Quadlet 컨테이너 (IPVLAN)
                   ↕ DRBD 동기화
[노드 2] DRBD Secondary ← 대기
```

내비게이션 바에서 순서대로 탭을 진행합니다:

| 탭 | 경로 | 출력물 |
|----|------|--------|
| **노드 풀** | `/nodes/` | 클러스터 노드, 네트워크 풀, Ansible 프로파일 |
| **DRBD 설정** | `/drbd/` | `.res` 리소스 파일 + Ansible 플레이북 |
| **Volume 설정** | `/volume/` | Named 볼륨 레지스트리 (DRBD 연동 또는 호스트 경로) |
| **Pod 설정** | `/quadlet/` | `.container` / `.pod` / `.volume` / `.network` systemd 유닛 파일 |
| **컨테이너 설정** | `/container/` | 컨테이너 유닛 파일 (폼 기반; 이미지, 볼륨, env, args) |
| **방화벽** | `/nft/` | nftables `netdev ingress` 필터 설정 |
| **Pacemaker 설정** | `/pacemaker/` | `pcs` 배시 스크립트 + Ansible 플레이북 |
| **클러스터 현황** | `/cluster/` | 실시간 토폴로지 뷰어 — 리소스, 제약조건, 노드 상태 |

서버 시작 시 `/etc/drbd.d/`, `/etc/containers/systemd/`, 로컬 pcsd REST API를 자동으로 스캔하여 노드 풀, 네트워크 풀, 클러스터 상태를 사전에 채웁니다.

---

## 사전 요구사항

모든 노드는 **AlmaLinux9 또는 RHEL9**이어야 합니다.

**각 클러스터 노드에 설치할 패키지:**

```bash
# ELRepo 활성화
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
firewall-cmd --permanent --add-port=7789/tcp   # DRBD 복제 (리소스 포트/범위에 맞게 조정)
firewall-cmd --permanent --add-port=5404-5405/udp   # Corosync
firewall-cmd --permanent --add-port=2224/tcp   # pcsd (노드 간 `pcs host auth` 및 이 앱의 pcsd REST 호출에 필요)
firewall-cmd --reload
```

**네트워크:**
- DRBD 복제 전용 NIC 권장
- IPVLAN 컨테이너 네트워킹을 위한 호스트 NIC 설정 필요

---

## 설정 파일

시작 디렉터리에서 `icm.conf`를 읽습니다. 파일이 없으면 기본값을 사용합니다.

```ini
[server]
host = 0.0.0.0   # 127.0.0.1로 설정하면 로컬호스트 전용
port = 5000

[logging]
level = info      # error | warn | info | debug | trace

[database]
path = ./icm.db

[paths]
drbd_dir    = /etc/drbd.d
quadlet_dir = /etc/containers/systemd
temp_dir    = /tmp/icm
deploy_mode = test   # "test" 모드는 temp_dir에 파일 생성

[pacemaker]
pcsd_url      = https://localhost:2224
pcsd_user     = hacluster
pcsd_password =
```

`RUST_LOG` 환경변수는 `[logging]` 섹션보다 우선합니다.

---

## 실행

```bash
git clone <repo-url>
cd ipvlan_container_manager
cargo build --release

# 필요 시 설정 편집
vi icm.conf

cargo run --release

# 로그 레벨 런타임 지정
RUST_LOG=debug cargo run
```

브라우저에서 `http://<host>:<port>` 접속 (기본값: `http://localhost:5000`).

---

## 사용 방법

### 노드 풀 (`/nodes/`)

클러스터 노드, 네트워크 정의, Ansible 배포 프로파일을 관리합니다. 이곳에 입력한 정보는 모든 탭에서 브라우저 localStorage를 통해 공유됩니다.

#### Ansible 프로파일

서버 측 Ansible 실행(인터페이스 수집, 배포)에 사용할 SSH 접속 정보를 저장합니다.

| 항목 | 설명 |
|------|------|
| 프로파일 이름 | 탭에서 선택하는 식별자 |
| 인증 방식 | `key` (SSH 개인키 경로) 또는 `password` |
| SSH 사용자 | 원격 접속 사용자 (기본값: `root`) |
| SSH 키 / 비밀번호 | 개인키 경로 또는 비밀번호 |
| become | `ansible_become` 활성화; 방식 및 become 비밀번호 |

하나의 프로파일을 **기본 Ansible 프로파일**로 지정하면 다른 모든 탭에서 자동으로 선택됩니다.

> **주의**: 비밀번호는 로컬 SQLite DB에 평문으로 저장됩니다. 테스트/랩 환경에서만 사용하세요.

#### 클러스터 노드

수동 추가/삭제 또는 **시스템 재스캔** 버튼으로 `/etc/drbd.d/` 및 pcsd API를 다시 읽습니다. 스캔으로 발견된 노드는 `scanned`, 수동 추가는 `manual`로 표시됩니다.

#### 네트워크 풀

컨테이너가 연결할 IPVLAN/macvlan 네트워크를 정의합니다. 호스트 인터페이스는 **인터페이스 자동감지** 기능으로 자동 매핑됩니다:

1. 네트워크 풀 카드 헤더의 **인터페이스 자동감지** 클릭
2. Ansible 프로파일 선택
3. **수집 시작** 클릭 — 서버가 Ansible 플레이북을 실행해 각 노드에서 `ip -j addr show` 결과를 수집하고, 네트워크의 서브넷과 인터페이스를 자동 매핑

Quadlet 생성 시 매핑이 없으면 `__PARENT_IFACE__` 플레이스홀더가 생성되고, Ansible 플레이북에 `ip route show` 태스크가 포함됩니다.

---

### 1단계 — DRBD 설정 (`/drbd/`)

| 항목 | 설명 | 예시 |
|------|------|------|
| 리소스 이름 | DRBD 리소스 이름 (`<name>.res` 파일) | `r0` |
| 프로토콜 | 동기화 방식 — HA에는 Protocol C 권장 | `C` |
| Minor 번호 | 디바이스 경로 `/dev/drbd<minor>` | `0` |
| 노드 호스트명 | FQDN 또는 호스트명 | `node1.ha.local` |
| 노드 IP | DRBD 복제에 사용되는 IP | `192.168.10.11` |
| 디스크 유형 | `block` (raw 디바이스) 또는 `lvm` | `block` |
| 포트 | DRBD 리슨 포트 | `7789` |

2~7 노드 지원. 고급 옵션(Net / Disk / Startup)은 접힌 패널에서 설정합니다.

**출력물:** DRBD `.res` 파일, 초기화 명령어, Ansible 플레이북.

---

### 2단계 — Volume 설정 (`/volume/`)

Pod/컨테이너에서 사용할 볼륨을 등록합니다.

| 항목 | 설명 |
|------|------|
| 이름 | Quadlet `.volume` 파일명 기반 |
| 호스트 경로 | 바인드 마운트 경로 (예: `/mnt/drbd/data`) |
| 설명 | 선택사항 |
| DRBD 리소스 | 이 볼륨과 연결되는 DRBD 리소스 |

등록된 볼륨은 컨테이너 설정 탭의 볼륨 마운트 드롭다운에 표시됩니다.

---

### 3단계 — Pod 설정 / Quadlet (`/quadlet/`)

#### 방법 A: 직접 입력

네트워크, 볼륨, Pod, 컨테이너 섹션을 직접 작성합니다. Pod 네트워크 항목에는 **IPv4 주소 (필수)** 와 **IPv6 주소 (선택)** 를 입력합니다.

#### 방법 B: podman inspect JSON

`podman inspect <컨테이너명>` 결과를 붙여넣으면 실행 중인 컨테이너 메타데이터를 분석해 Quadlet 유닛 파일을 생성합니다.

**생성 파일:**
- `<name>.network` — IPVLAN/macvlan 네트워크 (IPv4+IPv6 듀얼스택 지원)
- `<name>.volume` — Named 볼륨
- `<name>.pod` — Pod 그룹 유닛
- `<name>.container` — 컨테이너 서비스 유닛

양쪽 노드의 `/etc/containers/systemd/`에 배포 후 `systemctl daemon-reload` 실행.

#### CLI 대안

```bash
./scripts/podman_to_quadlet.sh --network ipvlan0 myapp mydb
./scripts/podman_to_quadlet.sh --install --network ipvlan0 myapp
```

필요 패키지: `podman`, `jq`

---

### 4단계 — 컨테이너 설정 (`/container/`)

폼 기반 UI로 컨테이너를 구성합니다:

- **컨테이너 이름** + **Pod** (스캔된 pod 드롭다운에서 선택)
- **이미지** URL
- **볼륨 마운트**: 등록된 볼륨 또는 named 볼륨 선택, 마운트 옵션 (`:z`, `:Z`, `:ro,z`, `:ro`)
- **환경변수**: 키-값 쌍
- **추가 인자**: 커맨드라인 인자

`.container` 유닛 파일과 Ansible 배포 플레이북을 생성합니다.

---

### 5단계 — 방화벽 / nftables (`/nft/`)

IPVLAN 컨테이너 인터페이스용 nftables `netdev ingress` 필터를 생성합니다.

**전역 설정:**
- **디바이스**: 수집된 인터페이스 드롭다운에서 호스트 NIC 선택
- **전역 규칙**: 프로토콜(tcp/udp) + 포트 범위 규칙 (예: traceroute UDP 33434–65535 허용)

**Pod별 규칙:**
- Pod를 드롭다운으로 선택하면 IP 주소가 자동 입력
- 서비스(포트) 및 소스 서브넷 그룹 설정

**출력물** — `.nft` 파일:
- Pod별 대상 IP 집합 (`target_<name>_v4`, `target_<name>_v6`)
- 서브넷 그룹 집합 (`sg_<name>_v4`, `sg_<name>_v6`)
- 서비스 포트 집합 + 허용/차단 규칙

---

### 6단계 — Pacemaker 설정 (`/pacemaker/`)

폼은 접을 수 있는 accordion 섹션으로 구성됩니다:

1. **클러스터 기본 설정** — 클러스터 이름, 노드 선택, Quorum 정책, STONITH
2. **DRBD + Filesystem** — DRBD 볼륨별 카드 (Promotable Clone + FS 리소스); 서비스당 여러 볼륨 지원
3. **리소스 그룹** — Pod + Container 그룹 (JSON)
4. **Systemd 리소스** — Quadlet 서비스 등록 (JSON)
5. **제약조건** — 자동 Order/Colocation + 수동 추가
6. **Ansible 배포 설정** — SSH 프로파일 및 대상 호스트

**pcsd 설정 가져오기** (폼 위 카드): pcsd URL 입력 후 가져오기 클릭 — 서버가 `/var/lib/pcsd/known-hosts`에서 노드 토큰을 읽어 현재 클러스터 리소스와 제약조건을 가져옵니다. 감지된 서비스 그룹마다 **"폼에 적용"** 버튼으로 자동 입력 가능. 전체 토폴로지는 **클러스터 현황** 탭을 이용하세요.

**자동 제약조건** (체크 시):
- **Order**: DRBD Clone이 Promote된 후 서비스 시작
- **Colocation**: 서비스가 DRBD Primary 노드에서만 실행

**출력물:** `pcs` 배시 스크립트, CIB XML 스니펫, Ansible 플레이북.

---

### 클러스터 현황 (`/cluster/`)

읽기 전용 토폴로지 뷰어. pcsd URL 입력 후 **클러스터 정보 가져오기** 클릭:

- order/colocation 제약조건 기반 Union-Find로 독립 서비스를 분리
- 각 서비스는 파이프라인으로 표시: **DRBD Clone → FS → Group → Systemd** (실행 노드 및 상태 배지 포함)
- DRBD Clone은 DB에 등록된 DRBD 리소스 이름과 대조하여 식별

---

## 배포

각 단계에서 Ansible 플레이북이 생성됩니다. 관리 서버에서 실행합니다:

```bash
ansible-playbook -i inventory.yml ansible/playbooks/deploy_drbd.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_quadlet.yml
ansible-playbook -i inventory.yml ansible/playbooks/deploy_pacemaker.yml
```

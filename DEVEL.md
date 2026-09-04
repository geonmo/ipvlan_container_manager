# DEVEL.md — 코드 구조 및 함수 레퍼런스

HA Container Manager 코드베이스의 상세 구조 문서입니다. 설치/사용법은
[README.md](README.md), 기능 개선 계획 및 검증 방법은 [PLAN.md](PLAN.md)를
참고하세요.

---

## 디렉토리 구조

```
ipvlan_container_manager/
├── src/
│   ├── main.rs                   # 진입점: 설정 로드, 라우터, Tera 초기화
│   ├── db.rs                     # SQLite CRUD 함수 + 스키마 정의
│   ├── scan.rs                   # 서버 시작 시 시스템 스캔 + 인터페이스 자동매핑
│   ├── nft_scanner.rs            # 기존 nftables 파일 파싱 → DB 동기화
│   ├── models/
│   │   ├── mod.rs
│   │   ├── drbd.rs               # DRBD 도메인 구조체
│   │   ├── quadlet.rs            # Quadlet 도메인 구조체 (PodNetworkEntry 포함)
│   │   ├── pacemaker.rs          # Pacemaker 도메인 구조체
│   │   └── nft.rs                # nftables 도메인 구조체
│   ├── generators/
│   │   ├── mod.rs
│   │   ├── drbd.rs               # .res 파일, Ansible 인벤토리/플레이북 생성
│   │   ├── quadlet.rs            # .container/.network/.pod/.volume 파일 생성
│   │   ├── pacemaker.rs          # pcs 스크립트, CIB XML, Ansible 플레이북 생성
│   │   └── nft.rs                # nftables netdev ingress 설정 생성
│   └── routes/
│       ├── mod.rs
│       ├── main.rs               # GET / (홈)
│       ├── drbd.rs               # DRBD 폼 + 결과 핸들러
│       ├── quadlet.rs            # Quadlet 폼 + 결과 핸들러
│       ├── pacemaker.rs          # Pacemaker 폼 + 결과 핸들러 + pcsd fetch API
│       ├── nft.rs                # 방화벽 폼 + nft 서브넷그룹/서비스/타겟/전역설정 API
│       ├── volume.rs             # Volume 관리 페이지 + REST API
│       ├── container.rs          # 컨테이너 설정 폼 + 결과 핸들러
│       └── nodes.rs              # 노드풀 API + Ansible 인터페이스 수집 + pod 스캔 API
├── templates/
│   ├── base.html                 # 공통 레이아웃 (Bootstrap 5 + Bootstrap Icons + CodeMirror)
│   ├── index.html                # 홈 화면
│   ├── drbd/
│   │   ├── index.html            # DRBD 입력 폼
│   │   └── result.html           # .res 파일 + Ansible 플레이북 결과
│   ├── quadlet/
│   │   ├── index.html            # Pod 설정 폼 (수동 + podman inspect)
│   │   ├── result.html           # 생성된 유닛 파일 + Ansible 플레이북
│   │   └── inspect_result.html   # podman inspect 결과
│   ├── pacemaker/
│   │   ├── index.html            # Pacemaker 폼 (pcsd 동기화 카드 포함)
│   │   └── result.html           # pcs 스크립트 + CIB XML
│   ├── nft/
│   │   ├── index.html            # 방화벽 폼 (전역 규칙 + pod별 규칙)
│   │   └── result.html           # 생성된 nftables 설정
│   ├── volume/
│   │   ├── index.html            # Volume 관리 페이지
│   │   └── result.html           # 생성된 .volume 파일
│   ├── container/
│   │   ├── index.html            # 컨테이너 설정 폼 (형식 기반 입력)
│   │   └── result.html           # 생성된 .container 파일
│   └── nodes/
│       └── index.html            # 노드풀/네트워크풀/Ansible 프로파일 관리
├── static/
│   ├── css/style.css             # 공통 CSS
│   └── js/main.js                # 공통 JS (localStorage 헬퍼, 프로파일 관리)
├── scripts/
│   └── podman_to_quadlet.sh      # bash 대안 (podman inspect → Quadlet)
├── ansible/
│   └── playbooks/                # 참고용 Ansible 플레이북
├── sample/                       # 수동 작성 예시 파일 (비교 참고용)
├── Cargo.toml
├── icm.conf                      # 설정 파일
├── CLAUDE.md
├── README.md
├── PLAN.md                       # 기능 개선 계획 + 검증/테스트 방법
└── DEVEL.md                      # 이 파일
```

---

## src/main.rs

애플리케이션 진입점. 설정 로드 → DB 초기화 → Tera 초기화 → 라우터 구성 → 서버 시작.

### 구조체

#### `AppState`
```rust
pub struct AppState {
    pub tera:        Arc<Tera>,              // 템플릿 엔진 (공유)
    pub db:          Arc<Mutex<Connection>>, // SQLite 커넥션 (Mutex로 보호)
    pub temp_dir:    String,                 // Ansible 임시 파일 경로
    pub deploy_mode: String,                 // "test" | "deploy"
}
```
Axum의 `State<AppState>` 추출자로 모든 핸들러에서 접근합니다.

#### `Config` (private)
`icm.conf`에서 로드되는 설정. `load_config()` 함수가 반환합니다.

### 주요 함수

| 함수 | 설명 |
|------|------|
| `load_config() -> Config` | `icm.conf` INI 파일 파싱. 파일 없으면 기본값 반환 |
| `filter_starts_with(value, args)` | Tera 커스텀 필터: `\| starts_with(pat="...")` |
| `filter_ends_with(value, args)` | Tera 커스텀 필터: `\| ends_with(pat="...")` |
| `main()` | tokio runtime 진입점. 설정 → DB → Tera → 라우터 → `axum::serve` |

### 백그라운드 스캔
```rust
tokio::spawn(async move {
    scan::startup_scan(scan_cfg, db_for_scan).await;
});
```
서버가 리슨을 시작한 직후 비동기로 실행됩니다.

---

## src/db.rs

SQLite 스키마와 CRUD 함수 모음. 모든 함수는 `&Connection`을 받아 동기적으로 실행됩니다.

### 스키마 (테이블별)

#### `nodes`
```sql
CREATE TABLE IF NOT EXISTS nodes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname   TEXT NOT NULL UNIQUE,
    ip         TEXT NOT NULL DEFAULT '',
    ssh_user   TEXT NOT NULL DEFAULT 'root',
    source     TEXT NOT NULL DEFAULT 'manual',
    created_at TEXT NOT NULL
);
```

#### `networks`
```sql
CREATE TABLE IF NOT EXISTS networks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    driver      TEXT NOT NULL DEFAULT 'ipvlan',
    interface   TEXT NOT NULL DEFAULT '',
    subnet      TEXT NOT NULL DEFAULT '',
    gateway     TEXT NOT NULL DEFAULT '',
    subnet6     TEXT NOT NULL DEFAULT '',
    gateway6    TEXT NOT NULL DEFAULT '',
    ipv6        INTEGER NOT NULL DEFAULT 0,
    ipvlan_mode TEXT NOT NULL DEFAULT 'l2',
    source      TEXT NOT NULL DEFAULT 'manual',
    created_at  TEXT NOT NULL
);
```

#### `ansible_profiles`
```sql
CREATE TABLE IF NOT EXISTS ansible_profiles (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL UNIQUE,
    auth_method     TEXT NOT NULL DEFAULT 'key',
    ssh_user        TEXT NOT NULL DEFAULT 'root',
    ssh_key         TEXT NOT NULL DEFAULT '~/.ssh/id_rsa',
    ssh_password    TEXT NOT NULL DEFAULT '',
    do_become       INTEGER NOT NULL DEFAULT 1,
    become_method   TEXT NOT NULL DEFAULT 'sudo',
    become_password TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL
);
```

#### `node_interfaces`
```sql
CREATE TABLE IF NOT EXISTS node_interfaces (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname   TEXT NOT NULL,
    interface  TEXT NOT NULL,
    ip         TEXT NOT NULL,
    prefix_len INTEGER NOT NULL DEFAULT 0,
    family     TEXT NOT NULL DEFAULT 'inet',
    created_at TEXT NOT NULL,
    UNIQUE(hostname, interface, ip)
);
```

#### `volumes`
```sql
CREATE TABLE IF NOT EXISTS volumes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT NOT NULL UNIQUE,
    host_path     TEXT NOT NULL DEFAULT '',
    description   TEXT NOT NULL DEFAULT '',
    drbd_resource TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL
);
```

#### nft 관련 테이블
```sql
CREATE TABLE IF NOT EXISTS nft_subnet_groups (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL UNIQUE,
    cidrs_v4_json TEXT NOT NULL DEFAULT '[]',
    cidrs_v6_json TEXT NOT NULL DEFAULT '[]',
    created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS nft_services (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL UNIQUE,
    tcp_ports_json  TEXT NOT NULL DEFAULT '[]',
    udp_ports_json  TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS nft_targets (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL UNIQUE,
    ipv4_addrs_json  TEXT NOT NULL DEFAULT '[]',
    ipv6_addrs_json  TEXT NOT NULL DEFAULT '[]',
    created_at       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS nft_target_rules (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    target_id         INTEGER NOT NULL,
    service_name      TEXT NOT NULL,
    subnet_group_name TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS nft_global_config (
    id               INTEGER PRIMARY KEY DEFAULT 1,
    table_name       TEXT NOT NULL DEFAULT 'filter_ingress',
    device_name      TEXT NOT NULL DEFAULT 'eth0',
    chain_name       TEXT NOT NULL DEFAULT 'ingress_eth0',
    traceroute_start INTEGER NOT NULL DEFAULT 33434,
    traceroute_end   INTEGER NOT NULL DEFAULT 65535,
    nft_file         TEXT NOT NULL DEFAULT '/etc/nftables/ipvlan_l2.nft',
    updated_at       TEXT NOT NULL DEFAULT ''
);
```

### DB 구조체

| 구조체 | 대응 테이블 | 특이사항 |
|--------|------------|---------|
| `DbNode` | `nodes` | `#[derive(Serialize, Deserialize)]` |
| `DbNetwork` | `networks` | `ipv6: bool` (sqlite INTEGER 0/1) |
| `DbAnsibleProfile` | `ansible_profiles` | `do_become` 필드에 `#[serde(rename = "become")]` |
| `DbNodeInterface` | `node_interfaces` | `family`: `"inet"` 또는 `"inet6"` |
| `DbVolume` | `volumes` | `drbd_resource`: 연결된 DRBD 리소스 이름 |
| `DbNftSubnetGroup` | `nft_subnet_groups` | `cidrs_v4`, `cidrs_v6`: `Vec<String>` ↔ JSON |
| `DbNftServiceDef` | `nft_services` | `tcp_ports`, `udp_ports`: `Vec<String>` ↔ JSON |
| `DbNftTarget` | `nft_targets` + `nft_target_rules` | rules: `Vec<DbNftTargetRule>` (JOIN) |
| `DbNftGlobalConfig` | `nft_global_config` | singleton row (id=1) |

---

## src/scan.rs

서버 시작 시 또는 수동 재스캔 시 실행되는 시스템 스캔 모듈.

### 공개 구조체

```rust
pub struct ScanConfig {
    pub drbd_dir:    String,
    pub quadlet_dir: String,
    pub pcsd_url:    String,
    pub pcsd_user:   String,
    pub pcsd_pass:   String,
    pub temp_dir:    String,
}

pub struct ScannedPodInfo {
    pub name:     String,
    pub networks: Vec<PodNetworkEntry>,
}
```

### 주요 함수

| 함수 | 설명 |
|------|------|
| `pub async fn startup_scan(cfg, db)` | 서버 시작 시 백그라운드로 실행. DRBD/네트워크/pcsd 스캔 + auto_match |
| `pub fn scan_drbd_nodes(dir) -> Vec<DbNode>` | `.res` 파일에서 `on <hostname>` 블록 파싱 |
| `pub fn scan_drbd_resources(dir) -> Vec<DrbdResInfo>` | `.res` 파일에서 리소스 이름 + 디스크 경로 추출 |
| `pub fn scan_quadlet_networks(dir) -> Vec<DbNetwork>` | `.network` 파일 파싱 (IPv4/IPv6 듀얼스택) |
| `pub fn scan_quadlet_pods(dir) -> Vec<ScannedPodInfo>` | `.pod` 파일 파싱. `Network=net.network:ip=X:ip6=Y` 파싱 |
| `pub async fn scan_pacemaker_nodes(url, user, pass) -> Vec<DbNode>` | pcsd REST API 또는 crm_mon 폴백 |
| `pub fn auto_match_network_interfaces(conn) -> usize` | node_interfaces ⊗ networks → update_network_interface |

`scan_quadlet_pods` 내부 헬퍼:
- `parse_pod_file(stem, content) -> ScannedPodInfo`
- `parse_network_param(param) -> PodNetworkEntry` — `"net.network:ip=X:ip6=Y"` 형태 파싱

---

## src/nft_scanner.rs

기존 시스템의 nftables 설정 파일을 파싱하여 DB에 동기화합니다.

### 공개 함수

```rust
pub async fn ensure_and_scan(
    temp_dir: &str,
    nft_file: &str,
    db: &Arc<Mutex<Connection>>,
    profile: Option<&DbAnsibleProfile>,
) -> anyhow::Result<NftScanResult>
```

처리 흐름:
1. `nft_file`이 로컬에 존재하면 바로 파싱
2. 없고 `profile`이 있으면 Ansible로 원격 파일 fetch
3. 파싱된 subnet groups, services, targets를 DB에 upsert

```rust
pub struct NftScanResult {
    pub message:          String,
    pub file_existed:     bool,
    pub groups_updated:   usize,
    pub services_updated: usize,
    pub targets_updated:  usize,
    pub ansible_output:   Option<String>,
}
```

---

## src/models/

### drbd.rs

| 구조체 | 설명 |
|--------|------|
| `DrbdNode` | 노드 정보. `disk_type`: `"block"` \| `"lvm"`. `effective_disk(resource_name)` 메서드 |
| `DrbdResource` | 전체 DRBD 리소스 설정. `nodes: Vec<DrbdNode>` (2~7개) |
| `DrbdNetOptions` | `allow_two_primaries`, split-brain 정책 |
| `DrbdDiskOptions` | `on_io_error`, `fencing` |
| `DrbdStartupOptions` | `wfc_timeout`, `degr_wfc_timeout`, `become_primary_on` |
| `AnsibleInventory` | DRBD 배포용 Ansible 인벤토리 설정 |

### quadlet.rs

| 구조체 | 설명 |
|--------|------|
| `PodNetworkEntry` | Pod 네트워크 항목. `ip: Option<String>` (IPv4, 필수), `ip6: Option<String>` (IPv6, 선택) |
| `QuadletVolume` | `.volume` 파일 데이터 |
| `QuadletNetwork` | `.network` 파일 데이터. 듀얼스택 지원. `interface` 비면 `__PARENT_IFACE__` |
| `QuadletPod` | `.pod` 파일 데이터. `networks: Vec<PodNetworkEntry>` |
| `QuadletContainer` | `.container` 파일 데이터 |
| `QuadletConfig` | 위 네 종류의 컬렉션 |

### pacemaker.rs

| 구조체 | 설명 |
|--------|------|
| `ClusterConfig` | 클러스터 전역 설정 |
| `DrbdPacemakerResource` | DRBD Promotable Clone 리소스 |
| `SystemdResource` | Quadlet systemd 서비스 리소스 |
| `OrderConstraint` | `pcs constraint order` |
| `ColocationConstraint` | `pcs constraint colocation` |
| `LocationConstraint` | `pcs constraint location` |
| `PacemakerConfig` | 위 모든 구조체의 컬렉션 |

### nft.rs

| 구조체 | 설명 |
|--------|------|
| `NftGlobalRule` | 전역 허용 규칙: `protocol` (tcp/udp), `port_start`, `port_end` |
| `NftSubnetGroup` | 서브넷 그룹: `name`, `cidrs_v4: Vec<String>`, `cidrs_v6: Vec<String>` |
| `NftServiceDef` | 서비스 정의: `name`, `tcp_ports: Vec<String>`, `udp_ports: Vec<String>` |
| `NftTargetRule` | 타겟별 허용 규칙: `service_name`, `subnet_group: Option<String>` |
| `NftTarget` | 타겟(Pod): `name`, `ipv4_addrs`, `ipv6_addrs`, `rules: Vec<NftTargetRule>` |
| `NftPolicy` | 전체 nftables 정책. `global_rules`, `targets`, `subnet_groups`, `services` |

---

## src/generators/

### drbd.rs

| 함수 | 설명 |
|------|------|
| `generate_res_file(resource) -> String` | DRBD `.res` 파일 텍스트 생성 |
| `generate_ansible_inventory(inventory) -> Result<String>` | YAML 인벤토리 생성 |
| `generate_ansible_playbook(resource, inventory) -> String` | DRBD 전체 배포 플레이북 |
| `generate_drbd_init_commands(resource) -> Vec<String>` | 수동 실행용 참고 명령어 |
| `parse_res_file(content, source_file) -> Option<ScannedDrbdResource>` | `.res` 파싱 |
| `scan_drbd_dir(dir) -> Vec<ScannedDrbdResource>` | 디렉토리 내 모든 `.res` 스캔 |

### quadlet.rs

| 함수 | 설명 |
|------|------|
| `generate_volume_unit(vol) -> String` | `.volume` 파일 생성 |
| `generate_network_unit(net) -> String` | `.network` 파일 생성. `interface` 비면 `__PARENT_IFACE__` |
| `generate_pod_unit(pod) -> String` | `.pod` 파일 생성. IPv4/IPv6 주소 모두 처리 |
| `generate_container_unit(c) -> String` | `.container` 파일 생성 |
| `generate_all_units(config) -> Vec<(String, String)>` | `(파일명, 내용)` 쌍 목록 |
| `podman_inspect_to_quadlet(json) -> Result<Vec<QuadletContainer>>` | `podman inspect` JSON → Quadlet |

`generate_pod_unit` IP 처리:
```rust
// ip, ip6 모두 있는 경우
Network=name.network:ip=192.168.1.10:ip6=2001::1
// ip만 있는 경우
Network=name.network:ip=192.168.1.10
// ip6만 있는 경우
Network=name.network:ip6=2001::1
// 둘 다 없는 경우
Network=name.network
```

### pacemaker.rs

| 함수 | 설명 |
|------|------|
| `generate_pcs_script(config) -> String` | 전체 `pcs` 명령 스크립트 |
| `generate_default_constraints(config: &mut PacemakerConfig)` | Order + Colocation 자동 생성 |
| `generate_cib_xml_snippet(config) -> String` | 참고용 CIB XML 조각 |

### nft.rs

| 함수 | 설명 |
|------|------|
| `generate_nft_policy(policy: &NftPolicy) -> String` | nftables 설정 파일 전체 생성 |

생성 순서:
1. `table netdev <name> {` 헤더
2. 섹션 1: 타겟 IP set (`target_<name>_v4/v6`)
3. 섹션 2: 참조된 서브넷 그룹 set (`sg_<name>_v4/v6`)
4. 섹션 3: 전역 규칙 포트 set (`global_<proto>_<idx>_ports`)
5. chain — ICMP + 전역 규칙 → 서비스별 규칙 → 대상별 drop 로그

헬퍼:
- `sanitize_set_name(name) -> String` — nft set 이름으로 사용 가능하도록 정규화
- `format_port_expr(ports) -> String` — 단일: `80`, 복수: `{ 80, 443 }`
- `format_inline_ips(ips) -> String` — 단일: `1.2.3.4`, 복수: `{ 1.2.3.4, 5.6.7.8 }`

---

## src/routes/

### nodes.rs

노드풀 관리 페이지와 REST API 핸들러.

#### 페이지/스캔
```rust
pub async fn index(State(state)) -> Html<String>
pub async fn rescan(State(state)) -> Result<Json<ScanResult>, StatusCode>
```

#### 노드/네트워크/프로파일/인터페이스 API
```rust
pub async fn api_list_nodes / api_upsert_node / api_delete_node
pub async fn api_list_networks / api_upsert_network / api_delete_network
pub async fn api_list_profiles / api_upsert_profile / api_delete_profile
pub async fn api_list_node_interfaces
pub async fn api_collect_interfaces(Json(input): Json<CollectInput>) -> ...
  // { profile_name: Option<String> }
  // Ansible 실행 → ip -j addr show → DB 저장 → auto_match
```

#### Pod 스캔 API
```rust
pub async fn api_list_quadlet_pods() -> Json<Vec<ScannedPodInfo>>
  // GET /api/quadlet-pods
  // scan_quadlet_pods("/etc/containers/systemd") 호출
```

### drbd.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /drbd/ | DRBD 폼 (저장된 리소스 목록 포함) |
| `scan` | GET /drbd/scan | `/etc/drbd.d/` 스캔 결과 JSON |
| `generate` | POST /drbd/generate | 폼 → generator → result.html |
| `download_res` | POST /drbd/download | `.res` 파일 직접 다운로드 |
| `save` | POST /drbd/save | DB 저장 |
| `delete_saved` | DELETE /drbd/delete/:name | DB 삭제 |
| `api_lvscan` | GET /api/lvscan | LVM 볼륨 로컬 스캔 |

### quadlet.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /quadlet/ | Pod 설정 폼 |
| `generate` | POST /quadlet/generate | `QuadletFullForm` → 유닛 파일 생성 |
| `from_inspect` | POST /quadlet/from-inspect | `podman inspect` JSON → Quadlet |

### pacemaker.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /pacemaker/ | Pacemaker 폼 (pcsd 동기화 카드 포함) |
| `generate` | POST /pacemaker/generate | `PacemakerFormData` → pcs 스크립트 생성 |
| `api_pcsd_fetch` | POST /api/pacemaker/pcsd-fetch | pcsd에서 클러스터 정보 가져오기 |

`api_pcsd_fetch` 처리 흐름:
```
1. PcsdFetchRequest { pcsd_url, pcsd_user, pcsd_pass } 수신
2. POST /api/v1/auth/login → 토큰 획득
3. GET /api/v1/cluster/status → 클러스터 상태
4. pcsd_find_resources() / pcsd_find_constraints() — 여러 JSON 경로 시도
5. { cluster_name, nodes, resources, constraints, raw_status } 반환
```

`pcsd_find_resources` / `pcsd_find_constraints`:
- pcsd 버전별로 JSON 구조가 다르므로 `/pacemaker/resources`, `/resources`, `/cluster_info/resources` 순으로 시도

### nft.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /nft/ | 방화벽 폼 |
| `generate` | POST /nft/generate | `NftGenerateForm` → nftables 설정 생성 + DB 저장 |
| `api_list/upsert/delete_subnet_group` | /api/nft/subnet-groups | 서브넷 그룹 CRUD |
| `api_list/upsert/delete_service` | /api/nft/services | 서비스 CRUD |
| `api_list/upsert/delete_target` | /api/nft/targets | 타겟 CRUD |
| `api_get/update_global_config` | /api/nft/config | 전역 설정 |
| `api_scan` | POST /api/nft/scan | 기존 nftables 파일 스캔 → DB 동기화 |

`NftGenerateForm` 주요 필드:
```rust
pub table_name:        Option<String>,
pub device_name:       Option<String>,
pub chain_name:        Option<String>,
pub global_rules_json: Option<String>,  // Vec<NftGlobalRule> JSON (없으면 traceroute 기본값)
pub targets_json:      String,           // Vec<NftTarget> JSON
```

### volume.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /volume/ | Volume 관리 페이지 |
| `generate` | POST /volume/generate | Volume 유닛 파일 생성 |
| `api_list` | GET /api/volumes | 볼륨 목록 |
| `api_upsert` | POST /api/volumes | 볼륨 추가/수정 |
| `api_delete` | DELETE /api/volumes/:id | 볼륨 삭제 |
| `api_list_drbd_resources` | GET /api/drbd/resources | 스캔된 DRBD 리소스 목록 |

### container.rs

| 핸들러 | 경로 | 설명 |
|--------|------|------|
| `index` | GET /container/ | 컨테이너 설정 폼 |
| `generate` | POST /container/generate | `containers_json` → `.container` 파일 생성 |
| `from_inspect` | POST /container/from-inspect | `podman inspect` JSON → 컨테이너 설정 |

---

## templates/

모든 템플릿은 `base.html`을 상속합니다 (`{% extends "base.html" %}`).

### 템플릿 블록
- `{% block title %}` — 페이지 제목
- `{% block nav_* %}active{% endblock %}` — 활성 네비게이션 항목
- `{% block content %}` — 본문 콘텐츠
- `{% block scripts %}` — 페이지별 JavaScript

### static/js/main.js — 공통 JS

#### localStorage 헬퍼
```javascript
const ICM_NODES_KEY    = 'icm_nodes';
const ICM_NETS_KEY     = 'icm_networks';
const ICM_PROFILES_KEY = 'icm_ansible_profiles';
const ICM_PROFILE_KEY  = 'icm_active_profile';  // 활성 프로파일 이름

function getNodePool()           // icm_nodes 읽기
function saveNodePool(arr)
function getNetworkPool()        // icm_networks 읽기
function saveNetworkPool(arr)
function getProfilePool()        // icm_ansible_profiles 읽기
function saveProfilePool(arr)
function getActiveProfileName()  // icm_active_profile 읽기
function setActiveProfileName(n) // icm_active_profile 쓰기
function getActiveProfile()      // 이름으로 프로파일 객체 조회
```

#### 공통 패턴: Ansible 프로파일 셀렉터

각 탭(Quadlet/Container/Pacemaker)은 동일한 패턴을 사용:
1. `getActiveProfileName()` 로 활성 프로파일 읽기
2. `getProfilePool()` 로 전체 프로파일 목록 읽어 `<select>` 채우기
3. 프로파일 선택 시: 수동 입력 필드 숨기고 hidden input에 프로파일 값 설정
4. 프로파일 미선택("직접 입력") 시: 수동 입력 필드 표시

### nodes/index.html 특이사항

서버 API를 직접 호출합니다 (`/api/*`). localStorage는 다른 탭 동기화 용도.

```
loadAll() → loadNodes() + loadNetworks() + loadProfiles() + loadNodeInterfaces()
         → syncLocalStorage() → 다른 탭의 셔틀 위젯 갱신
```

**기본 Ansible 프로파일 선택 UI**: `#activeProfileSel` 드롭다운. 변경 시 `ICM_PROFILE_KEY` 저장.

### nft/index.html 특이사항

- **device_name**: `/api/node-interfaces`에서 수집된 인터페이스 목록을 드롭다운으로 표시
- **전역 규칙**: `renderGlobalRules()` / `addGlobalRule()` / `syncGlobalRulesJson()` — 동적 행 관리, `global_rules_json` hidden input에 JSON 직렬화
- **타겟 Pod 선택**: `/api/quadlet-pods` 에서 스캔된 pod 목록 로드, pod 선택 시 네트워크 IP 자동 추출

### pacemaker/index.html 특이사항

**pcsd 동기화 카드** (폼 위):
- `pcsdFetch()` → `POST /api/pacemaker/pcsd-fetch` → `renderPcsdResult(data)`
- `renderResourceTable(items)` — 리소스 테이블 (ID, agent, clone/promotable 배지)
- `renderConstraintTable(constraints)` — Order/Colocation/Location 섹션별 테이블
- `applyPcsdNodes(nodes)` — 노드 셔틀에 pcsd 노드 추가

---

## 의존성 (Cargo.toml 주요 항목)

| 크레이트 | 버전 | 용도 |
|---------|------|------|
| `axum` | 0.7 | 웹 프레임워크. `State`, `Json`, `Form`, `Path` 추출자 |
| `tokio` | 1 | 비동기 런타임 |
| `tera` | 1 | Jinja2 호환 템플릿 엔진 |
| `rusqlite` | 0.31 (bundled) | SQLite 임베디드 DB |
| `serde` / `serde_json` | 1 | 직렬화/역직렬화 |
| `serde_yaml` | 0.9 | DRBD Ansible 인벤토리 YAML 생성 |
| `reqwest` | 0.12 (rustls-tls) | pcsd REST API HTTP 클라이언트. `danger_accept_invalid_certs(true)` |
| `tower-http` | 0.5 | 정적 파일 서빙 (`ServeDir`) |
| `tracing` / `tracing-subscriber` | 0.1 | 구조화 로깅 |
| `configparser` | 3 | `icm.conf` INI 파일 파싱 |
| `anyhow` | 1 | 에러 처리 |
| `chrono` | 0.4 | `created_at` 타임스탬프 생성 |

---

## 데이터 흐름 다이어그램

### 서버 시작 시 스캔
```
main()
  └─ tokio::spawn → startup_scan()
       ├─ scan_drbd_nodes(/etc/drbd.d)
       │    └─ parse .res files → upsert_node() × N
       ├─ scan_drbd_resources → insert_volume_if_not_exists() × N
       ├─ scan_quadlet_networks(/etc/containers/systemd)
       │    └─ parse .network files → upsert_network() × N
       ├─ scan_pacemaker_nodes(pcsd_url)
       │    └─ REST API or crm_mon fallback → upsert_node() × N
       └─ auto_match_network_interfaces()
            └─ node_interfaces ⊗ networks → update_network_interface() × M
```

### nftables 생성 흐름
```
POST /nft/generate
  └─ NftGenerateForm
       ├─ targets_json → Vec<NftTarget>
       ├─ global_rules_json → Vec<NftGlobalRule> (없으면 traceroute 기본값)
       ├─ DB: get_nft_subnet_groups_by_names()
       ├─ DB: get_nft_services_by_names()
       └─ generate_nft_policy(NftPolicy)
            ├─ 타겟 IP set (target_<name>_v4/v6)
            ├─ 서브넷 그룹 set (sg_<name>_v4/v6, 참조된 것만)
            ├─ 전역 포트 set (global_<proto>_<idx>_ports)
            └─ chain: ICMP + 전역 규칙 → 서비스 규칙 → drop 로그
                 └─ DB: upsert_nft_target() + update_nft_global_config()
```

### pcsd 동기화 흐름
```
POST /api/pacemaker/pcsd-fetch { pcsd_url, pcsd_user, pcsd_pass }
  └─ pcsd_fetch_cluster_info()
       ├─ POST /api/v1/auth/login → token
       ├─ GET /api/v1/cluster/status → raw JSON
       ├─ pcsd_find_resources() — 여러 경로 시도
       ├─ pcsd_find_constraints() — 여러 경로 시도
       └─ { cluster_name, nodes, resources, constraints, raw_status }
            └─ 프론트엔드: renderPcsdResult() → 테이블 표시 + 노드 적용 버튼
```

### 인터페이스 수집 흐름
```
POST /api/collect-interfaces { profile_name }
  └─ api_collect_interfaces()
       ├─ list_nodes() + list_ansible_profiles() from DB
       ├─ write_ansible_inventory() → /tmp/icm/collect_inventory.ini
       ├─ write_collect_playbook() → /tmp/icm/collect_interfaces.yml
       │    (ip -j addr show 실행 + temp_dir/ifaces_{{ inventory_hostname }}.json 저장)
       ├─ ansible-playbook 실행 (120s timeout)
       ├─ parse ifaces_<host>.json × N → parse_ip_addr_json()
       ├─ delete_node_interfaces_for_host() + upsert_node_interface() × N
       └─ auto_match_network_interfaces()
            └─ CollectResult { interfaces_imported, networks_matched, ansible_output }
```

use serde::{Deserialize, Serialize};

/// LINSTOR 스토리지 풀 — `linbit.linstor.storage_pool` role이 읽는
/// `linstor_storage_pools` 인벤토리 변수 항목과 1:1 대응한다
/// (`ansible-linstor-collection`의 `roles/storage_pool/tasks/main.yml`
/// 실제 소스를 확인해 필드 이름을 그대로 맞췄다).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinstorStoragePool {
    pub name: String,
    /// lvm | lvmthin | zfs | zfsthin | file | filethin
    pub pool_type: String,
    #[serde(default)]
    pub vg: Option<String>, // lvm / lvmthin
    #[serde(default)]
    pub vg_thinpool: Option<String>, // lvmthin
    #[serde(default)]
    pub zpool: Option<String>, // zfs / zfsthin
    #[serde(default)]
    pub file_path: Option<String>, // file / filethin
    #[serde(default)]
    pub physical_devices: Vec<String>, // lvm/lvmthin/zfs 백엔드 디바이스
    /// 이 풀을 적용할 satellite 노드 목록 (비어 있으면 전체 satellite 대상)
    #[serde(default)]
    pub nodes: Vec<String>,
}

/// LINSTOR 리소스 그룹 (`linbit.linstor.resource_group` 모듈)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinstorResourceGroup {
    pub name: String,
    pub storage_pool: String,
    pub place_count: u32,
}

/// LINSTOR 리소스 스폰 대상 (`linbit.linstor.resource` 모듈, `mode: spawn`)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinstorResourceSpawn {
    pub name: String,
    pub resource_group: String,
    /// 예: "100G"
    pub size: String,
}

fn default_true() -> bool {
    true
}

/// LINSTOR 배포 전체 설정 (PLAN.md D.2)
///
/// 이 앱이 직접 `linstor` CLI 명령을 조립하지 않는다 — LINBIT 공식
/// Ansible 컬렉션(`linbit.linstor` + `linbit.common`/`linbit.drbd`/
/// `linbit.drbd_reactor`)의 role/module을 그대로 호출하는 인벤토리 +
/// 플레이북만 생성한다.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinstorConfig {
    pub controller_nodes: Vec<String>,
    pub satellite_nodes: Vec<String>,
    /// gsdc-linbit-build(별도 저장소, PLAN.md D.3)가 만든 RPM들이 있는
    /// 제어 노드(control machine)의 로컬 디렉터리
    pub rpm_dir: String,
    #[serde(default)]
    pub storage_pools: Vec<LinstorStoragePool>,
    #[serde(default)]
    pub resource_groups: Vec<LinstorResourceGroup>,
    #[serde(default)]
    pub resources: Vec<LinstorResourceSpawn>,
    /// cluster_init_deploy_storage — linstor_storage_pools 변수로 스토리지 풀 생성
    #[serde(default = "default_true")]
    pub deploy_storage: bool,
    /// cluster_init_ha_database — 조건(결합노드 2+, 새틀라이트 3+) 미충족 시 자동 skip
    #[serde(default)]
    pub ha_database: bool,
    /// cluster_init_token_auth — REST API 토큰 인증 요구
    #[serde(default = "default_true")]
    pub token_auth: bool,
    /// 생성된 DRBD 리소스를 Pacemaker(`ocf:linbit:drbd`)가 관리할 것인가.
    ///
    /// 이 앱의 기본 전제다(Pacemaker 탭이 promotable clone을 만든다).
    /// 켜면 플레이북이 아래 전제조건을 함께 배포한다 — 3노드 실클러스터에서
    /// 이것들이 빠져 각각 다른 증상으로 실패하는 것을 확인했다:
    ///
    /// 1. `auto-promote no` — DRBD 9 기본값이 yes 이고 LINSTOR는 끄지 않는다.
    ///    켜져 있으면 장치를 여는 것만으로 커널이 Primary로 올려서 승격
    ///    시점을 통제해야 하는 Pacemaker와 어긋난다.
    /// 2. `drbd-selinux` — SELinux Enforcing에서 RA는 drbd_t 도메인으로 도는데,
    ///    이 정책 모듈이 없으면 drbdsetup이 커널과 통신할 netlink 소켓조차
    ///    만들지 못한다.
    /// 3. `/var/lib/linstor.d` 를 etc_t 로 라벨 — LINSTOR가 .res를 거기 쓰는데
    ///    var_lib_t 라서 drbd_t 가 읽지 못한다. 증상이 "DRBD resource ... not
    ///    found in configuration file /etc/drbd.conf." 라 원인과 동떨어져 보인다.
    #[serde(default = "default_true")]
    pub pacemaker_managed: bool,
}

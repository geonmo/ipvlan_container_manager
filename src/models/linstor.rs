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
}

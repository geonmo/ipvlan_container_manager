use serde::{Deserialize, Serialize};

/// Pacemaker 클러스터 기본 설정
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub cluster_name: String,
    pub nodes: Vec<ClusterNode>,
    pub stonith_enabled: bool,
    pub no_quorum_policy: String, // stop, freeze, ignore, suicide
    pub migration_threshold: u32,
    pub failure_timeout: String, // e.g. "60s"
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            cluster_name: "ha-cluster".to_string(),
            nodes: Vec::new(),
            stonith_enabled: false,
            no_quorum_policy: "stop".to_string(),
            migration_threshold: 3,
            failure_timeout: "60s".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub name: String,
    pub id: u32,
}

/// DRBD Promotable Clone 리소스
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdPacemakerResource {
    pub resource_name: String,    // e.g. "drbd-r0"
    pub drbd_resource_name: String, // DRBD .res 파일의 resource 이름
    pub clone_name: String,       // e.g. "drbd-r0-clone"
    pub notify: bool,
    pub promotable: bool,
    pub on_fail: String,          // fence, block, stop, ignore, demote
    pub target_role_master_node: Option<String>, // preferred primary node
    // op promote/demote/start/stop timeout (PLAN.md A.2 — R11.pacemaker_drbd_resources.yml
    // 실측 프리셋 기준 기본값: promote/demote=90s, start=240s, stop=100s).
    // notify/reload/monitor은 실측상 리소스마다 변하지 않아 생성기에서 상수로 고정한다.
    #[serde(default = "default_drbd_promote_timeout")]
    pub promote_timeout: String,
    #[serde(default = "default_drbd_demote_timeout")]
    pub demote_timeout: String,
    #[serde(default = "default_drbd_start_timeout")]
    pub start_timeout: String,
    #[serde(default = "default_drbd_stop_timeout")]
    pub stop_timeout: String,
}

fn default_drbd_promote_timeout() -> String { "90s".to_string() }
fn default_drbd_demote_timeout() -> String { "90s".to_string() }
fn default_drbd_start_timeout() -> String { "240s".to_string() }
fn default_drbd_stop_timeout() -> String { "100s".to_string() }

impl Default for DrbdPacemakerResource {
    fn default() -> Self {
        Self {
            resource_name: "drbd-r0".to_string(),
            drbd_resource_name: "r0".to_string(),
            clone_name: "drbd-r0-clone".to_string(),
            notify: true,
            promotable: true,
            on_fail: "fence".to_string(),
            target_role_master_node: None,
            promote_timeout: default_drbd_promote_timeout(),
            demote_timeout: default_drbd_demote_timeout(),
            start_timeout: default_drbd_start_timeout(),
            stop_timeout: default_drbd_stop_timeout(),
        }
    }
}

/// Systemd (Quadlet) 리소스
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemdResource {
    pub resource_name: String,       // pacemaker 리소스 이름
    pub systemd_unit: String,        // e.g. "myapp.service" or "mypod.service"
    pub resource_type: SystemdResourceType,
    pub monitor_interval: String,    // e.g. "30s"
    pub start_timeout: String,
    pub stop_timeout: String,
    pub clone: bool,                 // clone 리소스 여부
    pub clone_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SystemdResourceType {
    Container,
    Pod,
    Volume,
}

impl std::fmt::Display for SystemdResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SystemdResourceType::Container => write!(f, "Container"),
            SystemdResourceType::Pod => write!(f, "Pod"),
            SystemdResourceType::Volume => write!(f, "Volume"),
        }
    }
}

impl Default for SystemdResource {
    fn default() -> Self {
        Self {
            resource_name: String::new(),
            systemd_unit: String::new(),
            resource_type: SystemdResourceType::Container,
            monitor_interval: "30s".to_string(),
            start_timeout: "60s".to_string(),
            stop_timeout: "60s".to_string(),
            clone: false,
            clone_name: None,
        }
    }
}

/// 리소스 제약 조건 (Order)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderConstraint {
    pub id: String,
    pub first: String,
    pub first_action: String,  // start, promote, demote, stop
    pub then: String,
    pub then_action: String,
    pub kind: String,          // Mandatory, Optional, Serialize
}

/// 리소스 제약 조건 (Colocation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColocationConstraint {
    pub id: String,
    pub rsc: String,
    pub rsc_role: Option<String>,  // Master, Slave
    pub with_rsc: String,
    pub with_rsc_role: Option<String>,
    pub score: String,             // INFINITY, -INFINITY, or numeric
}

/// Location 제약 조건 (선호 노드)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationConstraint {
    pub id: String,
    pub rsc: String,
    pub node: String,
    pub score: String,
}

/// ocf:heartbeat:Filesystem 리소스 (DRBD 볼륨 위 FS 마운트)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsResource {
    pub resource_name: String,   // e.g. "fs-r0"
    pub device: String,          // e.g. "/dev/drbd/by-res/r0/0"
    pub directory: String,       // mountpoint e.g. "/mnt/drbd/r0"
    pub fstype: String,          // e.g. "xfs"
    pub monitor_interval: String,
    pub start_timeout: String,
    pub stop_timeout: String,
    /// 이 FS가 어느 DRBD clone에 귀속되는지 (colocation/order 자동생성용)
    pub drbd_clone_name: String,
}

impl Default for FsResource {
    fn default() -> Self {
        Self {
            resource_name: "fs-r0".to_string(),
            device: "/dev/drbd/by-res/r0/0".to_string(),
            directory: "/mnt/drbd/r0".to_string(),
            fstype: "xfs".to_string(),
            monitor_interval: "20s".to_string(),
            start_timeout: "60s".to_string(),
            stop_timeout: "60s".to_string(),
            drbd_clone_name: String::new(),
        }
    }
}

/// Pacemaker 리소스 그룹 (Pod + Container들을 순서대로 묶음)
/// 그룹 안에서 start는 members 순서대로, stop은 역순으로 진행됨
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceGroup {
    pub group_name: String,         // e.g. "grp-myapp"
    pub members: Vec<String>,       // 순서 있는 리소스 이름 목록 (pod → container들)
    /// 이 그룹이 어느 FS 리소스 뒤에 와야 하는지 (없으면 DRBD clone 직후)
    pub after_fs: Option<String>,
}

/// STONITH(fencing) 장치 — IPMI 기반 (PLAN.md A.5).
/// R02.service_pacemaker_stonith.yml/R55.se_backend_stonith.yml 실측 패턴:
/// `pcs stonith create stonith-ipmi-<node> fence_ipmilan pcmk_host_list=<node>
/// ip=<ipmi_ip> user=<ipmi_user> password=<ipmi_password> lanplus=1
/// power_wait=5 pcmk_reboot_timeout=300 pcmk_monitor_timeout=60
/// pcmk_reboot_action=reboot` — 파라미터 키가 `ip=`이지 `ipaddr=`가 아님에 주의.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StonithDevice {
    pub node: String,
    pub ipmi_ip: String,
    pub ipmi_user: String,
    pub ipmi_password: String,
    #[serde(default)]
    pub extra_opts: Vec<String>,
}

/// Pacemaker 전체 설정
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PacemakerConfig {
    pub cluster: ClusterConfig,
    pub drbd_resources: Vec<DrbdPacemakerResource>,
    pub fs_resources: Vec<FsResource>,          // DRBD 위 파일시스템 마운트
    pub systemd_resources: Vec<SystemdResource>, // Pod / Container 리소스
    pub resource_groups: Vec<ResourceGroup>,     // Pod+Container 그룹
    pub order_constraints: Vec<OrderConstraint>,
    pub colocation_constraints: Vec<ColocationConstraint>,
    pub location_constraints: Vec<LocationConstraint>,
    #[serde(default)]
    pub stonith_devices: Vec<StonithDevice>,
}

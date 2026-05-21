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
}

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
}

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

/// Pacemaker 전체 설정
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PacemakerConfig {
    pub cluster: ClusterConfig,
    pub drbd_resources: Vec<DrbdPacemakerResource>,
    pub systemd_resources: Vec<SystemdResource>,
    pub order_constraints: Vec<OrderConstraint>,
    pub colocation_constraints: Vec<ColocationConstraint>,
    pub location_constraints: Vec<LocationConstraint>,
}

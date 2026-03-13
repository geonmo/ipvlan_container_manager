use serde::{Deserialize, Serialize};

/// DRBD 노드 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdNode {
    pub hostname: String,
    pub ip: String,
    pub disk_device: String, // e.g. /dev/sdb
    pub meta_disk: String,   // e.g. internal
    pub port: u16,
}

impl Default for DrbdNode {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            ip: String::new(),
            disk_device: String::new(),
            meta_disk: "internal".to_string(),
            port: 7789,
        }
    }
}

/// DRBD 리소스 설정
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdResource {
    pub resource_name: String,
    pub protocol: String,         // A, B, C
    pub minor: u32,
    pub nodes: Vec<DrbdNode>,
    /// net 섹션 옵션
    pub net_options: DrbdNetOptions,
    /// disk 섹션 옵션
    pub disk_options: DrbdDiskOptions,
    /// startup 섹션 옵션
    pub startup_options: DrbdStartupOptions,
}

impl Default for DrbdResource {
    fn default() -> Self {
        Self {
            resource_name: "r0".to_string(),
            protocol: "C".to_string(),
            minor: 0,
            nodes: vec![DrbdNode::default(), DrbdNode::default()],
            net_options: DrbdNetOptions::default(),
            disk_options: DrbdDiskOptions::default(),
            startup_options: DrbdStartupOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdNetOptions {
    pub allow_two_primaries: bool,
    pub after_sb_0pri: String,
    pub after_sb_1pri: String,
    pub after_sb_2pri: String,
}

impl Default for DrbdNetOptions {
    fn default() -> Self {
        Self {
            allow_two_primaries: false,
            after_sb_0pri: "discard-younger-primary".to_string(),
            after_sb_1pri: "discard-secondary".to_string(),
            after_sb_2pri: "disconnect".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdDiskOptions {
    pub on_io_error: String,
    pub fencing: String,
}

impl Default for DrbdDiskOptions {
    fn default() -> Self {
        Self {
            on_io_error: "passthrough".to_string(),
            fencing: "resource-only".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrbdStartupOptions {
    pub wfc_timeout: u32,
    pub degr_wfc_timeout: u32,
    pub become_primary_on: String, // "both" for dual-primary or specific node
}

impl Default for DrbdStartupOptions {
    fn default() -> Self {
        Self {
            wfc_timeout: 15,
            degr_wfc_timeout: 60,
            become_primary_on: String::new(),
        }
    }
}

/// Ansible 인벤토리 설정
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnsibleInventory {
    pub nodes: Vec<AnsibleNode>,
    pub ansible_user: String,
    pub ansible_ssh_private_key_file: String,
    pub r#become: bool,
}

impl Default for AnsibleInventory {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            ansible_user: "root".to_string(),
            ansible_ssh_private_key_file: "~/.ssh/id_rsa".to_string(),
            r#become: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnsibleNode {
    pub hostname: String,
    pub ip: String,
}

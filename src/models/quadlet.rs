use serde::{Deserialize, Serialize};

/// Quadlet Volume 유닛
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletVolume {
    pub name: String,        // e.g. "data"  → data.volume
    pub driver: String,      // e.g. "local"
    pub options: Vec<String>, // e.g. ["type=nfs", "o=addr=..."]
    pub labels: Vec<String>,
}

/// Quadlet Network 유닛 (IPVLAN)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletNetwork {
    pub name: String,        // e.g. "ipvlan0"  → ipvlan0.network
    pub driver: String,      // ipvlan or macvlan
    pub interface: String,   // host NIC e.g. eth0
    pub subnet: String,      // e.g. 192.168.1.0/24
    pub gateway: String,
    pub ipvlan_mode: String, // l2, l3, l3s
    pub options: Vec<String>,
}

/// Quadlet Pod 유닛
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletPod {
    pub name: String,        // e.g. "mypod"  → mypod.pod
    pub publish_ports: Vec<String>,
    pub network: Option<String>,
    pub labels: Vec<String>,
}

/// Quadlet Container 유닛
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuadletContainer {
    pub name: String,           // e.g. "myapp" → myapp.container
    pub image: String,
    pub pod: Option<String>,    // pod 소속 여부
    pub network: Option<String>,
    pub ip: Option<String>,     // 고정 IP
    pub environment: Vec<(String, String)>,
    pub volumes: Vec<String>,   // "volname:/path:options"
    pub publish_ports: Vec<String>,
    pub exec: Option<String>,
    pub user: Option<String>,
    pub labels: Vec<String>,
    pub extra_args: Vec<String>,
    pub depends_on: Vec<String>, // 다른 container 이름
    pub drbd_resource: Option<String>, // 연결된 DRBD 리소스
}

impl Default for QuadletContainer {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            pod: None,
            network: None,
            ip: None,
            environment: Vec::new(),
            volumes: Vec::new(),
            publish_ports: Vec::new(),
            exec: None,
            user: None,
            labels: Vec::new(),
            extra_args: Vec::new(),
            depends_on: Vec::new(),
            drbd_resource: None,
        }
    }
}

/// Quadlet 전체 설정
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletConfig {
    pub volumes: Vec<QuadletVolume>,
    pub networks: Vec<QuadletNetwork>,
    pub pods: Vec<QuadletPod>,
    pub containers: Vec<QuadletContainer>,
    pub install_path: String, // /etc/containers/systemd/
}

impl QuadletConfig {
    pub fn new() -> Self {
        Self {
            install_path: "/etc/containers/systemd".to_string(),
            ..Default::default()
        }
    }
}

/// podman ps 출력 파싱용 구조체
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PodmanContainerInfo {
    pub id: String,
    pub names: String,
    pub image: String,
    pub ports: String,
    pub status: String,
    pub command: String,
    pub mounts: Vec<String>,
    pub env: Vec<String>,
    pub network_settings: Option<PodmanNetworkInfo>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PodmanNetworkInfo {
    pub network_name: String,
    pub ip_address: String,
}

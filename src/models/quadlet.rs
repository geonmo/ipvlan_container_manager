use serde::{Deserialize, Serialize};

/// Quadlet Volume 유닛
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletVolume {
    pub name: String,        // e.g. "data"  → data.volume
    pub driver: String,      // e.g. "local"
    pub options: Vec<String>, // e.g. ["type=nfs", "o=addr=..."]
    pub labels: Vec<String>,
}

/// Quadlet Network 유닛 (IPVLAN / macvlan)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletNetwork {
    pub name: String,        // e.g. "ipvlan0"  → ipvlan0.network
    pub driver: String,      // ipvlan or macvlan
    pub interface: String,   // host NIC e.g. eth0
    pub subnet: String,      // IPv4 서브넷 e.g. 192.168.1.0/24
    pub gateway: String,     // IPv4 게이트웨이
    pub subnet6: String,     // IPv6 서브넷 (없으면 빈 문자열)
    pub gateway6: String,    // IPv6 게이트웨이 (없으면 빈 문자열)
    pub ipv6: bool,          // IPv6= 라인 출력 여부
    pub ipvlan_mode: String, // l2, l3, l3s
    pub internal: bool,      // Internal=true (외부 접근 차단)
    pub options: Vec<String>,
}

/// Pod에 연결할 네트워크 엔트리 (복수 네트워크 지원)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PodNetworkEntry {
    pub network: String,
    #[serde(default)]
    pub ip: Option<String>,      // IPv4 주소
    #[serde(default)]
    pub ip6: Option<String>,     // IPv6 주소 (선택)
    #[serde(default)]
    pub gateway: Option<String>, // IPv4 게이트웨이 (선택, pod Network= 인라인)
    #[serde(default)]
    pub gateway6: Option<String>,// IPv6 게이트웨이 (선택, pod Network= 인라인)
}

/// Quadlet Pod 유닛
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuadletPod {
    pub name: String,                     // e.g. "mypod"  → mypod.pod
    #[serde(default)]
    pub description: Option<String>,      // [Unit] Description=
    #[serde(default)]
    pub hostname: Option<String>,         // HostName=
    #[serde(default)]
    pub shm_size: Option<String>,         // ShmSize=
    pub networks: Vec<PodNetworkEntry>,   // 복수 네트워크 지원
    pub labels: Vec<String>,
}

/// Quadlet Container 유닛
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuadletContainer {
    pub name: String,           // e.g. "myapp" → myapp.container
    pub image: String,
    #[serde(default)]
    pub description: Option<String>,       // [Unit] Description=
    pub pod: Option<String>,               // pod 소속 여부
    pub network: Option<String>,
    pub ip: Option<String>,                // 고정 IP
    pub environment: Vec<(String, String)>,
    pub volumes: Vec<String>,              // "volname:/path:options"
    #[serde(default)]
    pub tmpfs: Vec<String>,                // Tmpfs= 마운트
    pub publish_ports: Vec<String>,
    pub exec: Option<String>,
    pub entrypoint: Option<String>,        // Entrypoint=
    pub user: Option<String>,
    #[serde(default)]
    pub auto_update: Option<String>,       // AutoUpdate= (e.g. "registry")
    pub labels: Vec<String>,
    pub extra_args: Vec<String>,
    pub depends_on: Vec<String>,           // 다른 container/pod 이름
    pub drbd_resource: Option<String>,     // 연결된 DRBD 리소스
    // [Service] 설정
    #[serde(default)]
    pub service_type: Option<String>,      // e.g. "oneshot"
    #[serde(default)]
    pub remain_after_exit: Option<bool>,   // RemainAfterExit=
    #[serde(default)]
    pub timeout_stop_sec: Option<String>,  // TimeoutStopSec=
    // [Install] / 권한 (PLAN.md C)
    #[serde(default)]
    pub wanted_by: Option<String>,         // [Install] WantedBy=
    #[serde(default)]
    pub add_capabilities: Vec<String>,     // AddCapability=
}

impl Default for QuadletContainer {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            description: None,
            pod: None,
            network: None,
            ip: None,
            environment: Vec::new(),
            volumes: Vec::new(),
            tmpfs: Vec::new(),
            publish_ports: Vec::new(),
            exec: None,
            entrypoint: None,
            user: None,
            auto_update: None,
            labels: Vec::new(),
            extra_args: Vec::new(),
            depends_on: Vec::new(),
            drbd_resource: None,
            service_type: None,
            remain_after_exit: None,
            timeout_stop_sec: None,
            wanted_by: None,
            add_capabilities: Vec::new(),
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

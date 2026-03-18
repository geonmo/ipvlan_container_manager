use serde::{Deserialize, Serialize};

/// DB: 서브넷 그룹 (firewalld zone 유사)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NftSubnetGroup {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub cidrs_v4: Vec<String>,
    #[serde(default)]
    pub cidrs_v6: Vec<String>,
}

/// DB: 서비스 포트 정의 (firewalld service 유사)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NftServiceDef {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tcp_ports: Vec<String>,
    #[serde(default)]
    pub udp_ports: Vec<String>,
}

/// 생성용: 단일 허용 규칙 엔트리
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftTargetRule {
    pub service_name: String,
    /// None = 모든 소스 허용 (open), Some = 해당 서브넷 그룹만 허용
    #[serde(default)]
    pub subnet_group: Option<String>,
}

/// 생성용: 단일 pod/컨테이너 대상
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftTarget {
    pub name: String,
    #[serde(default)]
    pub ipv4_addrs: Vec<String>,
    #[serde(default)]
    pub ipv6_addrs: Vec<String>,
    #[serde(default)]
    pub rules: Vec<NftTargetRule>,
}

/// 전역 허용 규칙 (모든 대상 IP에 적용, 프로토콜 + 포트 범위)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftGlobalRule {
    pub protocol:   String, // "tcp" | "udp"
    pub port_start: u16,
    pub port_end:   u16,
}

/// 생성기 입력: 전체 nftables 정책
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftPolicy {
    pub filename: String,
    pub table_name: String,
    pub device_name: String,
    pub chain_name: String,
    pub traceroute_start: u16,
    pub traceroute_end: u16,
    /// 전역 허용 규칙 목록 (있으면 traceroute_start/end 대신 사용)
    #[serde(default)]
    pub global_rules: Vec<NftGlobalRule>,
    pub targets: Vec<NftTarget>,
    /// DB에서 확장된 서브넷 그룹
    pub subnet_groups: Vec<NftSubnetGroup>,
    /// DB에서 확장된 서비스 정의
    pub services: Vec<NftServiceDef>,
}

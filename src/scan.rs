//! 서버 시작 시 시스템 정보를 스캔하여 DB에 저장합니다.
//!
//! 1. `/etc/drbd.d/*.res` → 클러스터 노드 (hostname + IP)
//! 2. `/etc/containers/systemd/*.network` → Quadlet 네트워크 풀
//! 3. pcsd REST API (또는 crm_mon -X 폴백) → Pacemaker 노드 목록

use std::path::Path;
use std::sync::{Arc, Mutex};
use rusqlite::Connection;
use tracing::{info, warn, debug};

use crate::db::{
    DbNode, DbNetwork, DbNodeInterface, DbVolume,
    upsert_node, upsert_network, update_network_interface,
    list_node_interfaces, list_networks, insert_volume_if_not_exists,
};

// ─── DRBD .res 파일 파서 ──────────────────────────────────────────────────

/// /etc/drbd.d/ 디렉토리를 스캔해서 노드 정보를 추출합니다.
pub fn scan_drbd_nodes(drbd_dir: &str) -> Vec<DbNode> {
    let dir = Path::new(drbd_dir);
    if !dir.exists() {
        debug!("DRBD 디렉토리 없음: {}", drbd_dir);
        return vec![];
    }

    let mut nodes: std::collections::HashMap<String, DbNode> = std::collections::HashMap::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => { warn!("DRBD 디렉토리 읽기 실패: {}", e); return vec![]; }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("res") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => { warn!("파일 읽기 실패 {:?}: {}", path, e); continue; }
        };
        parse_res_for_nodes(&content, &mut nodes);
    }

    let result: Vec<DbNode> = nodes.into_values().collect();
    info!("DRBD 스캔 완료: 노드 {}개 발견", result.len());
    result
}

fn parse_res_for_nodes(content: &str, nodes: &mut std::collections::HashMap<String, DbNode>) {
    let mut in_on_block = false;
    let mut current_hostname = String::new();
    let mut current_ip = String::new();
    let mut brace_depth: i32 = 0;
    let mut on_block_depth: i32 = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // `on <hostname> {` 블록 시작
        if !in_on_block {
            if let Some(rest) = trimmed.strip_prefix("on ") {
                let hostname = rest.trim_end_matches('{').trim().to_string();
                if !hostname.is_empty() && !hostname.starts_with('#') {
                    in_on_block = true;
                    current_hostname = hostname;
                    current_ip = String::new();
                    on_block_depth = brace_depth + 1;
                }
            }
        }

        // 중괄호 깊이 추적
        for ch in trimmed.chars() {
            if ch == '{' { brace_depth += 1; }
            else if ch == '}' { brace_depth -= 1; }
        }

        if in_on_block {
            // `address ip:port;` 파싱
            if trimmed.starts_with("address ") {
                let addr_part = trimmed
                    .trim_start_matches("address ")
                    .trim_end_matches(';')
                    .trim();
                // IPv4: 1.2.3.4:port
                if let Some(ip) = addr_part.split(':').next() {
                    current_ip = ip.trim().to_string();
                }
            }

            // 블록 종료
            if brace_depth < on_block_depth {
                in_on_block = false;
                if !current_hostname.is_empty() {
                    nodes.entry(current_hostname.clone()).or_insert(DbNode {
                        id: 0,
                        hostname: current_hostname.clone(),
                        ip: current_ip.clone(),
                        ssh_user: "root".to_string(),
                        source: "scanned".to_string(),
                        created_at: String::new(),
                    });
                }
            }
        }
    }
}

// ─── DRBD .res 리소스 + 디스크 정보 파서 ─────────────────────────────────

/// .res 파일 하나의 요약 정보
pub struct DrbdResInfo {
    pub name:      String, // resource <name>
    pub disk_path: String, // disk 라인에서 추출 (예: /dev/vg_data/lv_data)
}

/// /etc/drbd.d/ 의 .res 파일을 스캔하여 리소스 이름 + 디스크 경로를 반환합니다.
/// 각 노드의 LVM 볼륨 이름/크기는 동일하다고 가정하므로 첫 번째 `disk` 라인만 사용합니다.
pub fn scan_drbd_resources(drbd_dir: &str) -> Vec<DrbdResInfo> {
    let dir = Path::new(drbd_dir);
    if !dir.exists() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e)  => e,
        Err(e) => { warn!("DRBD 디렉토리 읽기 실패: {}", e); return vec![]; }
    };
    let mut resources = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("res") { continue; }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c)  => c,
            Err(e) => { warn!("파일 읽기 실패 {:?}: {}", path, e); continue; }
        };
        if let Some(info) = parse_res_for_resource(&stem, &content) {
            resources.push(info);
        }
    }
    info!("DRBD 리소스 스캔 완료: {}개 (디스크 정보 있음)", resources.len());
    resources
}

fn parse_res_for_resource(stem: &str, content: &str) -> Option<DrbdResInfo> {
    let mut res_name  = stem.to_string();
    let mut disk_path = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        // resource <name> {
        if let Some(rest) = trimmed.strip_prefix("resource ") {
            let name = rest.trim_end_matches('{').trim().to_string();
            if !name.is_empty() { res_name = name; }
        }
        // disk /dev/vg/lv;  — 첫 번째 등장만 사용 (모든 노드에서 동일하다고 가정)
        if disk_path.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("disk ") {
                let p = rest.trim_end_matches(';').trim().to_string();
                if !p.is_empty() && p != "none" {
                    disk_path = p;
                }
            }
        }
    }

    if disk_path.is_empty() { return None; }
    Some(DrbdResInfo { name: res_name, disk_path })
}

// ─── Quadlet .network 파일 파서 ───────────────────────────────────────────

/// /etc/containers/systemd/*.network 파일을 스캔해서 네트워크 풀을 추출합니다.
pub fn scan_quadlet_networks(quadlet_dir: &str) -> Vec<DbNetwork> {
    let dir = Path::new(quadlet_dir);
    if !dir.exists() {
        debug!("Quadlet 디렉토리 없음: {}", quadlet_dir);
        return vec![];
    }

    let mut networks = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => { warn!("Quadlet 디렉토리 읽기 실패: {}", e); return vec![]; }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("network") {
            continue;
        }
        let stem = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => { warn!("파일 읽기 실패 {:?}: {}", path, e); continue; }
        };
        if let Some(net) = parse_network_file(&stem, &content) {
            networks.push(net);
        }
    }

    info!("Quadlet 스캔 완료: 네트워크 {}개 발견", networks.len());
    networks
}

fn parse_network_file(name: &str, content: &str) -> Option<DbNetwork> {
    let mut driver = String::new();
    let mut interface = String::new();
    let mut subnets: Vec<String> = Vec::new();
    let mut gateways: Vec<String> = Vec::new();
    let mut ipvlan_mode = "l2".to_string();
    let mut ipv6 = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("Driver=") {
            driver = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Subnet=") {
            subnets.push(v.trim().to_string());
        } else if let Some(v) = trimmed.strip_prefix("Gateway=") {
            gateways.push(v.trim().to_string());
        } else if let Some(v) = trimmed.strip_prefix("Options=parent=") {
            interface = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Options=mode=") {
            ipvlan_mode = v.trim().to_string();
        } else if trimmed.eq_ignore_ascii_case("IPv6=true")
               || trimmed.eq_ignore_ascii_case("IPv6=yes") {
            ipv6 = true;
        }
    }

    if driver.is_empty() { return None; }

    // IPv4 / IPv6 분류: ':' 포함이면 IPv6
    let mut subnet4 = String::new();
    let mut gateway4 = String::new();
    let mut subnet6 = String::new();
    let mut gateway6 = String::new();

    for s in &subnets {
        if s.contains(':') { subnet6 = s.clone(); }
        else               { subnet4 = s.clone(); }
    }
    for g in &gateways {
        if g.contains(':') { gateway6 = g.clone(); }
        else               { gateway4 = g.clone(); }
    }
    if !subnet6.is_empty() { ipv6 = true; }

    Some(DbNetwork {
        id: 0,
        name: name.to_string(),
        driver,
        interface,
        subnet:  subnet4,
        gateway: gateway4,
        subnet6,
        gateway6,
        ipv6,
        ipvlan_mode,
        source: "scanned".to_string(),
        created_at: String::new(),
    })
}

// ─── Quadlet .pod 파일 파서 ───────────────────────────────────────────────

use crate::models::quadlet::PodNetworkEntry;

/// 스캔된 Pod 정보
#[derive(Debug, serde::Serialize)]
pub struct ScannedPodInfo {
    pub name:     String,
    pub networks: Vec<PodNetworkEntry>,
}

/// /etc/containers/systemd/*.pod 파일을 스캔해서 Pod + 네트워크 설정을 반환합니다.
pub fn scan_quadlet_pods(quadlet_dir: &str) -> Vec<ScannedPodInfo> {
    let dir = Path::new(quadlet_dir);
    if !dir.exists() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e)  => e,
        Err(e) => { warn!("Quadlet 디렉토리 읽기 실패: {}", e); return vec![]; }
    };
    let mut pods = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("pod") { continue; }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c)  => c,
            Err(e) => { warn!("파일 읽기 실패 {:?}: {}", path, e); continue; }
        };
        pods.push(parse_pod_file(&stem, &content));
    }
    info!("Quadlet Pod 스캔 완료: {}개", pods.len());
    pods
}

fn parse_pod_file(stem: &str, content: &str) -> ScannedPodInfo {
    let mut name = stem.to_string();
    let mut networks: Vec<PodNetworkEntry> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("PodName=") {
            name = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Network=") {
            // e.g. "ipvlan0.network:ip=192.168.1.10:ip6=2001::1"
            let entry = parse_network_param(v.trim());
            networks.push(entry);
        }
    }

    ScannedPodInfo { name, networks }
}

/// "netname.network:ip=X:ip6=Y" → PodNetworkEntry
fn parse_network_param(param: &str) -> PodNetworkEntry {
    let mut parts = param.splitn(2, ':');
    let net_part = parts.next().unwrap_or("").trim_end_matches(".network").to_string();
    let rest = parts.next().unwrap_or("");

    let mut ip: Option<String>  = None;
    let mut ip6: Option<String> = None;

    for kv in rest.split(':') {
        if let Some(v) = kv.strip_prefix("ip=") {
            ip = Some(v.to_string());
        } else if let Some(v) = kv.strip_prefix("ip6=") {
            ip6 = Some(v.to_string());
        }
    }

    PodNetworkEntry { network: net_part, ip, ip6 }
}

// ─── Pacemaker 노드 스캔 ──────────────────────────────────────────────────

/// pcsd REST API 또는 crm_mon 폴백으로 Pacemaker 노드 목록을 가져옵니다.
pub async fn scan_pacemaker_nodes(
    pcsd_url: &str,
    pcsd_user: &str,
    pcsd_pass: &str,
) -> Vec<DbNode> {
    if pcsd_pass.is_empty() {
        debug!("pcsd_password 미설정 — Pacemaker 스캔 건너뜀");
        return try_crm_mon_fallback().await;
    }

    match try_pcsd_api(pcsd_url, pcsd_user, pcsd_pass).await {
        Ok(nodes) => {
            info!("pcsd API 스캔 완료: 노드 {}개", nodes.len());
            nodes
        }
        Err(e) => {
            warn!("pcsd API 스캔 실패: {} — crm_mon 폴백 시도", e);
            try_crm_mon_fallback().await
        }
    }
}

async fn try_pcsd_api(pcsd_url: &str, user: &str, pass: &str) -> anyhow::Result<Vec<DbNode>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    // 1단계: 로그인하여 토큰 획득
    let login_url = format!("{}/api/v1/auth/login", pcsd_url.trim_end_matches('/'));
    let login_body = serde_json::json!({
        "username": user,
        "password": pass
    });
    let login_resp = client.post(&login_url)
        .json(&login_body)
        .send()
        .await?;

    if !login_resp.status().is_success() {
        return Err(anyhow::anyhow!("pcsd 로그인 실패: {}", login_resp.status()));
    }

    let login_data: serde_json::Value = login_resp.json().await?;
    let token = login_data["token"].as_str()
        .ok_or_else(|| anyhow::anyhow!("pcsd 토큰 없음"))?
        .to_string();

    // 2단계: 클러스터 노드 목록 조회
    let status_url = format!("{}/api/v1/cluster/status", pcsd_url.trim_end_matches('/'));
    let status_resp = client.get(&status_url)
        .bearer_auth(&token)
        .send()
        .await?;

    if !status_resp.status().is_success() {
        return Err(anyhow::anyhow!("클러스터 상태 조회 실패: {}", status_resp.status()));
    }

    let data: serde_json::Value = status_resp.json().await?;

    // cluster/status 응답에서 노드 추출 (pcsd 버전에 따라 구조 다름)
    let mut nodes = Vec::new();
    if let Some(arr) = data["cluster_settings"]["nodes"].as_array() {
        for node in arr {
            if let Some(name) = node["name"].as_str() {
                nodes.push(DbNode {
                    id: 0,
                    hostname: name.to_string(),
                    ip: node["addr"].as_str().unwrap_or("").to_string(),
                    ssh_user: "root".to_string(),
                    source: "scanned".to_string(),
                    created_at: String::new(),
                });
            }
        }
    }

    Ok(nodes)
}

/// `crm_mon --output-as=xml` 으로 Pacemaker 노드 목록을 가져옵니다.
async fn try_crm_mon_fallback() -> Vec<DbNode> {
    let output = match tokio::process::Command::new("crm_mon")
        .args(["--output-as=xml", "--one-shot"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => { debug!("crm_mon 실행 실패: {}", e); return vec![]; }
    };

    if !output.status.success() {
        debug!("crm_mon 종료 코드 비정상");
        return vec![];
    }

    let xml = String::from_utf8_lossy(&output.stdout);
    parse_crm_mon_xml(&xml)
}

fn parse_crm_mon_xml(xml: &str) -> Vec<DbNode> {
    // 간단한 정규식 없이 문자열 파싱으로 <node name="..." ...> 추출
    let mut nodes = Vec::new();
    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<node ") {
            if let Some(name) = extract_xml_attr(trimmed, "name") {
                nodes.push(DbNode {
                    id: 0,
                    hostname: name,
                    ip: String::new(),
                    ssh_user: "root".to_string(),
                    source: "scanned".to_string(),
                    created_at: String::new(),
                });
            }
        }
    }
    info!("crm_mon 폴백 스캔 완료: 노드 {}개", nodes.len());
    nodes
}

fn extract_xml_attr(s: &str, attr: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr);
    let start = s.find(&pattern)? + pattern.len();
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

// ─── 통합 스타트업 스캔 ───────────────────────────────────────────────────

pub struct ScanConfig {
    pub drbd_dir:    String,
    pub quadlet_dir: String,
    pub pcsd_url:    String,
    pub pcsd_user:   String,
    pub pcsd_pass:   String,
    pub temp_dir:    String,
}

/// 서버 시작 시 모든 스캔을 수행하고 DB에 저장합니다.
pub async fn startup_scan(config: ScanConfig, db: Arc<Mutex<Connection>>) {
    info!("시스템 스캔 시작...");

    // 1. DRBD 노드 스캔
    let drbd_nodes = scan_drbd_nodes(&config.drbd_dir);
    {
        let conn = db.lock().unwrap();
        let mut count = 0;
        for node in &drbd_nodes {
            if upsert_node(&conn, node).is_ok() { count += 1; }
        }
        info!("DRBD 노드 {}개 DB 저장 완료", count);
    }

    // 1b. DRBD 리소스 → Volume 자동 등록 (이미 등록된 경우 건너뜀)
    let drbd_resources = scan_drbd_resources(&config.drbd_dir);
    {
        let conn = db.lock().unwrap();
        let mut count = 0;
        for res in &drbd_resources {
            let vol = DbVolume {
                id:            0,
                name:          res.name.clone(),
                host_path:     format!("/mnt/drbd/{}", res.name),
                description:   res.disk_path.clone(),
                drbd_resource: res.name.clone(),
                created_at:    String::new(),
            };
            if insert_volume_if_not_exists(&conn, &vol).is_ok() { count += 1; }
        }
        info!("DRBD 볼륨 {}개 DB 등록 완료 (신규)", count);
    }

    // 2. Quadlet 네트워크 스캔
    let networks = scan_quadlet_networks(&config.quadlet_dir);
    {
        let conn = db.lock().unwrap();
        let mut count = 0;
        for net in &networks {
            if upsert_network(&conn, net).is_ok() { count += 1; }
        }
        info!("Quadlet 네트워크 {}개 DB 저장 완료", count);
    }

    // 3. Pacemaker 노드 스캔 (비동기)
    let pm_nodes = scan_pacemaker_nodes(
        &config.pcsd_url,
        &config.pcsd_user,
        &config.pcsd_pass,
    ).await;

    {
        let conn = db.lock().unwrap();
        let mut count = 0;
        for node in &pm_nodes {
            if upsert_node(&conn, node).is_ok() { count += 1; }
        }
        info!("Pacemaker 노드 {}개 DB 저장 완료", count);
    }

    info!("시스템 스캔 완료");

    // 4. node_interfaces → networks 인터페이스 자동매칭
    auto_match_network_interfaces(&db.lock().unwrap());

    // 5. nft 파일 스캔
    let nft_file = {
        let conn = db.lock().unwrap();
        match crate::db::get_nft_global_config(&conn) {
            Ok(cfg) => cfg.nft_file,
            Err(_)  => "/etc/nftables/ipvlan_l2.nft".to_string(),
        }
    };
    match crate::nft_scanner::ensure_and_scan(&config.temp_dir, &nft_file, &db, None).await {
        Ok(result) => info!("nft 스캔: {}", result.message),
        Err(e)     => warn!("nft 스캔 실패: {}", e),
    }
}

// ─── IP 서브넷 매칭 ────────────────────────────────────────────────────────

fn ip4_in_subnet(ip: &str, subnet: &str) -> bool {
    let (net_s, prefix_s) = match subnet.split_once('/') {
        Some(v) => v,
        None    => return false,
    };
    let prefix: u32 = match prefix_s.parse() {
        Ok(v)  => v,
        Err(_) => return false,
    };
    let a: u32 = match net_s.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.into(),
        Err(_) => return false,
    };
    let b: u32 = match ip.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.into(),
        Err(_) => return false,
    };
    if prefix == 0 { return true; }
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    (a & mask) == (b & mask)
}

fn ip6_in_subnet(ip: &str, subnet: &str) -> bool {
    let (net_s, prefix_s) = match subnet.split_once('/') {
        Some(v) => v,
        None    => return false,
    };
    let prefix: u32 = match prefix_s.parse() {
        Ok(v)  => v,
        Err(_) => return false,
    };
    let a: u128 = match net_s.parse::<std::net::Ipv6Addr>() {
        Ok(ip) => ip.into(),
        Err(_) => return false,
    };
    let b: u128 = match ip.parse::<std::net::Ipv6Addr>() {
        Ok(ip) => ip.into(),
        Err(_) => return false,
    };
    if prefix == 0 { return true; }
    let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
    (a & mask) == (b & mask)
}

/// node_interfaces 테이블에서 IP를 읽어 networks 테이블의 interface 필드를 자동 갱신.
/// 반환값: 갱신된 네트워크 수
pub fn auto_match_network_interfaces(conn: &Connection) -> usize {
    let interfaces = match list_node_interfaces(conn) {
        Ok(v) => v,
        Err(e) => { warn!("node_interfaces 로드 실패: {}", e); return 0; }
    };
    if interfaces.is_empty() { return 0; }

    let networks = match list_networks(conn) {
        Ok(v) => v,
        Err(e) => { warn!("networks 로드 실패: {}", e); return 0; }
    };

    let mut matched = 0usize;
    for net in &networks {
        let found = find_interface_for_network(&interfaces, net);
        if let Some(iface) = found {
            if iface != net.interface {
                if update_network_interface(conn, &net.name, &iface).is_ok() {
                    info!("네트워크 '{}' 인터페이스 자동매칭: {}", net.name, iface);
                    matched += 1;
                }
            }
        }
    }
    matched
}

/// interfaces 목록에서 net의 서브넷에 속하는 IP를 가진 인터페이스를 반환.
/// 여러 노드에서 다른 인터페이스 이름이 나오면 경고 후 첫 번째 반환.
fn find_interface_for_network(interfaces: &[DbNodeInterface], net: &DbNetwork) -> Option<String> {
    let mut candidates: Vec<&str> = Vec::new();

    for iface in interfaces {
        let matched = if iface.family == "inet" && !net.subnet.is_empty() {
            ip4_in_subnet(&iface.ip, &net.subnet)
        } else if iface.family == "inet6" && !net.subnet6.is_empty() {
            ip6_in_subnet(&iface.ip, &net.subnet6)
        } else {
            false
        };
        if matched {
            candidates.push(&iface.interface);
        }
    }

    if candidates.is_empty() { return None; }

    // 모든 후보가 같은 이름인지 확인
    let first = candidates[0];
    if candidates.iter().any(|&c| c != first) {
        warn!(
            "네트워크 '{}': 노드마다 인터페이스 이름이 다릅니다 ({:?}) — 첫 번째 사용",
            net.name, candidates
        );
    }
    Some(first.to_string())
}

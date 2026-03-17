//! 서버 시작 시 시스템 정보를 스캔하여 DB에 저장합니다.
//!
//! 1. `/etc/drbd.d/*.res` → 클러스터 노드 (hostname + IP)
//! 2. `/etc/containers/systemd/*.network` → Quadlet 네트워크 풀
//! 3. pcsd REST API (또는 crm_mon -X 폴백) → Pacemaker 노드 목록

use std::path::Path;
use std::sync::{Arc, Mutex};
use rusqlite::Connection;
use tracing::{info, warn, debug};

use crate::db::{DbNode, DbNetwork, upsert_node, upsert_network};

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
    let mut subnet = String::new();
    let mut gateway = String::new();
    let mut ipvlan_mode = "l2".to_string();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("Driver=") {
            driver = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Subnet=") {
            subnet = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Gateway=") {
            gateway = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Options=parent=") {
            interface = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Options=mode=") {
            ipvlan_mode = v.trim().to_string();
        }
    }

    if driver.is_empty() { return None; }

    Some(DbNetwork {
        id: 0,
        name: name.to_string(),
        driver,
        interface,
        subnet,
        gateway,
        ipvlan_mode,
        source: "scanned".to_string(),
        created_at: String::new(),
    })
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
}

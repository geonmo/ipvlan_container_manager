use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Json},
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::db::{
    DbNode, DbNetwork, DbAnsibleProfile, DbNodeInterface, DbVolume, DbStorageBackend,
    list_nodes, upsert_node, delete_node,
    list_networks, upsert_network, delete_network,
    list_ansible_profiles, upsert_ansible_profile, delete_ansible_profile,
    list_node_interfaces, upsert_node_interface, delete_node_interfaces_for_host,
    insert_volume_if_not_exists,
    get_storage_backend, set_storage_backend,
};
use crate::scan::{self, auto_match_network_interfaces};

// ─── 페이지 ────────────────────────────────────────────────────────────────

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("nodes/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─── 시스템 재스캔 (DRBD/Quadlet 파일) ────────────────────────────────────

#[derive(Serialize)]
pub struct ScanResult {
    pub nodes_added:    usize,
    pub networks_added: usize,
}

pub async fn rescan(State(state): State<AppState>) -> Result<Json<ScanResult>, StatusCode> {
    let drbd_nodes     = scan::scan_drbd_nodes("/etc/drbd.d");
    let drbd_resources = scan::scan_drbd_resources("/etc/drbd.d");
    let networks       = scan::scan_quadlet_networks("/etc/containers/systemd");
    let mut nodes_added    = 0usize;
    let mut networks_added = 0usize;
    {
        let conn = crate::lock_db(&state.db);
        for node in &drbd_nodes {
            if upsert_node(&conn, node).is_ok() { nodes_added += 1; }
        }
        for net in &networks {
            if upsert_network(&conn, net).is_ok() { networks_added += 1; }
        }
        for res in &drbd_resources {
            let vol = DbVolume {
                id:            0,
                name:          res.name.clone(),
                host_path:     format!("/mnt/drbd/{}", res.name),
                description:   res.disk_path.clone(),
                drbd_resource: res.name.clone(),
                created_at:    String::new(),
            };
            let _ = insert_volume_if_not_exists(&conn, &vol);
        }
        auto_match_network_interfaces(&conn);
    }
    Ok(Json(ScanResult { nodes_added, networks_added }))
}

// ─── Quadlet Pod 스캔 API ──────────────────────────────────────────────────

pub async fn api_list_quadlet_pods() -> Json<Vec<crate::scan::ScannedPodInfo>> {
    Json(crate::scan::scan_quadlet_pods("/etc/containers/systemd"))
}

// ─── 노드 API ──────────────────────────────────────────────────────────────

pub async fn api_list_nodes(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbNode>>, StatusCode> {
    let conn = crate::lock_db(&state.db);
    Ok(Json(list_nodes(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

#[derive(Deserialize)]
pub struct NodeInput {
    pub hostname: String,
    pub ip:       Option<String>,
    pub ssh_user: Option<String>,
}

pub async fn api_upsert_node(
    State(state): State<AppState>,
    Json(input): Json<NodeInput>,
) -> Result<StatusCode, StatusCode> {
    let node = DbNode {
        id: 0,
        hostname: input.hostname,
        ip:       input.ip.unwrap_or_default(),
        ssh_user: input.ssh_user.unwrap_or_else(|| "root".to_string()),
        source:   "manual".to_string(),
        created_at: String::new(),
    };
    let conn = crate::lock_db(&state.db);
    upsert_node(&conn, &node).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn api_delete_node(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = crate::lock_db(&state.db);
    delete_node(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

// ─── 네트워크 API ──────────────────────────────────────────────────────────

pub async fn api_list_networks(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbNetwork>>, StatusCode> {
    let conn = crate::lock_db(&state.db);
    Ok(Json(list_networks(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

#[derive(Deserialize)]
pub struct NetworkInput {
    pub name:        String,
    pub driver:      Option<String>,
    pub subnet:      Option<String>,
    pub gateway:     Option<String>,
    pub subnet6:     Option<String>,
    pub gateway6:    Option<String>,
    pub ipv6:        Option<bool>,
    pub ipvlan_mode: Option<String>,
}

pub async fn api_upsert_network(
    State(state): State<AppState>,
    Json(input): Json<NetworkInput>,
) -> Result<StatusCode, StatusCode> {
    let subnet6  = input.subnet6.unwrap_or_default();
    let gateway6 = input.gateway6.unwrap_or_default();
    let ipv6     = input.ipv6.unwrap_or(!subnet6.is_empty());
    let net = DbNetwork {
        id:          0,
        name:        input.name,
        driver:      input.driver.unwrap_or_else(|| "ipvlan".to_string()),
        interface:   String::new(),   // 항상 자동감지
        subnet:      input.subnet.unwrap_or_default(),
        gateway:     input.gateway.unwrap_or_default(),
        subnet6,
        gateway6,
        ipv6,
        ipvlan_mode: input.ipvlan_mode.unwrap_or_else(|| "l2".to_string()),
        source:      "manual".to_string(),
        created_at:  String::new(),
    };
    let conn = crate::lock_db(&state.db);
    upsert_network(&conn, &net).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // 저장 직후 자동매칭 시도
    auto_match_network_interfaces(&conn);
    Ok(StatusCode::OK)
}

pub async fn api_delete_network(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = crate::lock_db(&state.db);
    delete_network(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

// ─── Ansible 프로파일 API ──────────────────────────────────────────────────

pub async fn api_list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbAnsibleProfile>>, StatusCode> {
    let conn = crate::lock_db(&state.db);
    Ok(Json(list_ansible_profiles(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

#[derive(Deserialize)]
pub struct AnsibleProfileInput {
    pub name:            String,
    pub auth_method:     Option<String>,
    pub ssh_user:        Option<String>,
    pub ssh_key:         Option<String>,
    pub ssh_password:    Option<String>,
    #[serde(rename = "become")]
    pub do_become:       Option<bool>,
    pub become_method:   Option<String>,
    pub become_password: Option<String>,
}

pub async fn api_upsert_profile(
    State(state): State<AppState>,
    Json(input): Json<AnsibleProfileInput>,
) -> Result<StatusCode, StatusCode> {
    let profile = DbAnsibleProfile {
        id:              0,
        name:            input.name,
        auth_method:     input.auth_method.unwrap_or_else(|| "key".to_string()),
        ssh_user:        input.ssh_user.unwrap_or_else(|| "root".to_string()),
        ssh_key:         input.ssh_key.unwrap_or_else(|| "~/.ssh/id_rsa".to_string()),
        ssh_password:    input.ssh_password.unwrap_or_default(),
        do_become:       input.do_become.unwrap_or(true),
        become_method:   input.become_method.unwrap_or_else(|| "sudo".to_string()),
        become_password: input.become_password.unwrap_or_default(),
        created_at:      String::new(),
        updated_at:      String::new(),
    };
    let conn = crate::lock_db(&state.db);
    upsert_ansible_profile(&conn, &profile).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn api_delete_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = crate::lock_db(&state.db);
    delete_ansible_profile(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

// ─── 노드 인터페이스 API ───────────────────────────────────────────────────

pub async fn api_list_node_interfaces(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbNodeInterface>>, StatusCode> {
    let conn = crate::lock_db(&state.db);
    Ok(Json(list_node_interfaces(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

// ─── 네트워크 인터페이스 수집 (Ansible 실행) ──────────────────────────────

#[derive(Deserialize)]
pub struct CollectInput {
    pub profile_name: Option<String>,
}

#[derive(Serialize)]
pub struct CollectResult {
    pub interfaces_imported: usize,
    pub networks_matched:    usize,
    pub ansible_output:      String,
}

pub async fn api_collect_interfaces(
    State(state): State<AppState>,
    Json(input): Json<CollectInput>,
) -> Result<Json<CollectResult>, (StatusCode, String)> {
    let temp_dir = state.temp_dir.clone();
    let e500 = |m: String| (StatusCode::INTERNAL_SERVER_ERROR, m);

    // DB에서 노드와 프로파일 로드
    let (nodes, profile) = {
        let conn = crate::lock_db(&state.db);
        let nodes = list_nodes(&conn).map_err(|e| e500(e.to_string()))?;
        if nodes.is_empty() {
            return Err((StatusCode::BAD_REQUEST,
                "No nodes to collect from. Add nodes to the node pool first.".to_string()));
        }
        let all_profiles = list_ansible_profiles(&conn).map_err(|e| e500(e.to_string()))?;
        let profile = if let Some(ref name) = input.profile_name {
            all_profiles.into_iter().find(|p| &p.name == name)
        } else {
            all_profiles.into_iter().next()
        };
        (nodes, profile)
    };

    // 임시 디렉토리 생성
    std::fs::create_dir_all(&temp_dir).map_err(|e| e500(format!("creating temp_dir: {}", e)))?;

    // Ansible 인벤토리 + 플레이북 생성
    let inv_path = format!("{}/collect_inventory.ini", temp_dir);
    let pb_path  = format!("{}/collect_interfaces.yml", temp_dir);
    write_ansible_inventory(&nodes, &profile, &inv_path)
        .map_err(|e| e500(format!("creating the inventory: {}", e)))?;
    write_collect_playbook(&temp_dir, &pb_path)
        .map_err(|e| e500(format!("creating the playbook: {}", e)))?;

    // ansible-playbook 실행
    let mut cmd = tokio::process::Command::new("ansible-playbook");
    cmd.arg("-i").arg(&inv_path).arg(&pb_path)
       .arg("--ssh-extra-args=-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null")
       .stdout(std::process::Stdio::piped())
       .stderr(std::process::Stdio::piped());

    if let Some(ref p) = profile {
        if p.auth_method == "key" && !p.ssh_key.is_empty() {
            let key = expand_home(&p.ssh_key);
            cmd.arg("--private-key").arg(&key);
        }
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        cmd.output(),
    ).await
     .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "Ansible timed out (120s)".into()))?
     .map_err(|e| e500(format!("failed to run ansible-playbook: {}\n(check that ansible-playbook is installed)", e)))?;

    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    let ansible_output = format!("{}{}", stdout, stderr);

    if !result.status.success() {
        return Err((StatusCode::INTERNAL_SERVER_ERROR,
            format!("Ansible failed:\n{}", ansible_output)));
    }

    // 결과 파일 파싱 및 DB 저장
    let mut imported = 0usize;
    {
        let conn = crate::lock_db(&state.db);
        for node in &nodes {
            let file = format!("{}/ifaces_{}.json", temp_dir, node.hostname);
            match std::fs::read_to_string(&file) {
                Ok(content) => {
                    match parse_ip_addr_json(&node.hostname, &content) {
                        Ok(ifaces) => {
                            delete_node_interfaces_for_host(&conn, &node.hostname).ok();
                            for iface in &ifaces {
                                if upsert_node_interface(&conn, iface).is_ok() {
                                    imported += 1;
                                }
                            }
                        }
                        Err(e) => tracing::warn!("failed to parse node {}: {}", node.hostname, e),
                    }
                }
                Err(e) => tracing::warn!("failed to read file {}: {}", file, e),
            }
        }
        let matched = auto_match_network_interfaces(&conn);
        return Ok(Json(CollectResult {
            interfaces_imported: imported,
            networks_matched:    matched,
            ansible_output,
        }));
    }
}

// ─── 스토리지 백엔드 감지 (PLAN.md D.1) ────────────────────────────────────
//
// linbit.linstor 공식 컬렉션의 controller_install/satellite_install role이
// 실제로 쓰는 것과 동일한 판정 방식(systemd 유닛 파일 존재 여부)을 그대로
// 재사용한다 — `roles/controller_install/tasks/main.yml`의
// `/usr/lib/systemd/system/linstor-controller.service` stat 확인,
// `roles/satellite_install/tasks/main.yml`의
// `/usr/lib/systemd/system/linstor-satellite.service` stat 확인과 동일한
// 경로. 사용자 결정에 따라 노드별 혼재는 다루지 않고, 클러스터 내 한 노드
// 라도 컨트롤러/새틀라이트 유닛이 있으면 전역적으로 "linstor"로 판정한다.

pub async fn api_get_storage_backend(
    State(state): State<AppState>,
) -> Result<Json<DbStorageBackend>, StatusCode> {
    let conn = crate::lock_db(&state.db);
    Ok(Json(get_storage_backend(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

#[derive(Serialize)]
pub struct StorageBackendDetectResult {
    pub backend: String,
    pub detected_at: String,
    /// 노드별 감지 상세 (hostname, linstor-controller 유닛 존재, linstor-satellite 유닛 존재)
    pub per_node: Vec<(String, bool, bool)>,
    pub ansible_output: String,
}

pub async fn api_detect_storage_backend(
    State(state): State<AppState>,
    Json(input): Json<CollectInput>,
) -> Result<Json<StorageBackendDetectResult>, (StatusCode, String)> {
    let temp_dir = state.temp_dir.clone();
    let e500 = |m: String| (StatusCode::INTERNAL_SERVER_ERROR, m);

    let (nodes, profile) = {
        let conn = crate::lock_db(&state.db);
        let nodes = list_nodes(&conn).map_err(|e| e500(e.to_string()))?;
        if nodes.is_empty() {
            return Err((StatusCode::BAD_REQUEST,
                "No nodes to detect on. Add nodes to the node pool first.".to_string()));
        }
        let all_profiles = list_ansible_profiles(&conn).map_err(|e| e500(e.to_string()))?;
        let profile = if let Some(ref name) = input.profile_name {
            all_profiles.into_iter().find(|p| &p.name == name)
        } else {
            all_profiles.into_iter().next()
        };
        (nodes, profile)
    };

    std::fs::create_dir_all(&temp_dir).map_err(|e| e500(format!("creating temp_dir: {}", e)))?;

    let inv_path = format!("{}/linstor_detect_inventory.ini", temp_dir);
    let pb_path  = format!("{}/linstor_detect.yml", temp_dir);
    write_ansible_inventory(&nodes, &profile, &inv_path)
        .map_err(|e| e500(format!("creating the inventory: {}", e)))?;
    write_storage_backend_detect_playbook(&temp_dir, &pb_path)
        .map_err(|e| e500(format!("creating the playbook: {}", e)))?;

    let mut cmd = tokio::process::Command::new("ansible-playbook");
    cmd.arg("-i").arg(&inv_path).arg(&pb_path)
       .arg("--ssh-extra-args=-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null")
       .stdout(std::process::Stdio::piped())
       .stderr(std::process::Stdio::piped());

    if let Some(ref p) = profile {
        if p.auth_method == "key" && !p.ssh_key.is_empty() {
            let key = expand_home(&p.ssh_key);
            cmd.arg("--private-key").arg(&key);
        }
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        cmd.output(),
    ).await
     .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "Ansible timed out (120s)".into()))?
     .map_err(|e| e500(format!("failed to run ansible-playbook: {}\n(check that ansible-playbook is installed)", e)))?;

    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    let ansible_output = format!("{}{}", stdout, stderr);

    if !result.status.success() {
        return Err((StatusCode::INTERNAL_SERVER_ERROR,
            format!("Ansible failed:\n{}", ansible_output)));
    }

    let mut per_node = Vec::new();
    for node in &nodes {
        let file = format!("{}/linstor_detect_{}.json", temp_dir, node.hostname);
        match std::fs::read_to_string(&file) {
            Ok(content) => {
                let v: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
                let has_ctrl = v["controller"].as_bool().unwrap_or(false);
                let has_sat  = v["satellite"].as_bool().unwrap_or(false);
                per_node.push((node.hostname.clone(), has_ctrl, has_sat));
            }
            Err(e) => tracing::warn!("failed to read the detection result for node {}: {}", node.hostname, e),
        }
    }

    let backend = if per_node.iter().any(|(_, ctrl, sat)| *ctrl || *sat) {
        "linstor"
    } else {
        "kernel-module"
    };

    let detected_at = {
        let conn = crate::lock_db(&state.db);
        set_storage_backend(&conn, backend).map_err(|e| e500(e.to_string()))?;
        get_storage_backend(&conn).map_err(|e| e500(e.to_string()))?.detected_at
    };

    Ok(Json(StorageBackendDetectResult {
        backend: backend.to_string(),
        detected_at,
        per_node,
        ansible_output,
    }))
}

fn write_storage_backend_detect_playbook(temp_dir: &str, path: &str) -> anyhow::Result<()> {
    let content = format!(r#"---
- name: Detect whether LINSTOR is installed (PLAN.md D.1)
  hosts: all
  gather_facts: no
  tasks:
    - name: Check for the linstor-controller systemd unit
      ansible.builtin.stat:
        path: /usr/lib/systemd/system/linstor-controller.service
      register: _linstor_ctrl_stat

    - name: Check for the linstor-satellite systemd unit
      ansible.builtin.stat:
        path: /usr/lib/systemd/system/linstor-satellite.service
      register: _linstor_sat_stat

    - name: Save the detection result locally
      local_action:
        module: copy
        content: "{{{{ {{'controller': _linstor_ctrl_stat.stat.exists, 'satellite': _linstor_sat_stat.stat.exists}} | to_json }}}}"
        dest: "{temp_dir}/linstor_detect_{{{{ inventory_hostname }}}}.json"
        mode: '0600'
"#, temp_dir = temp_dir);
    std::fs::write(path, content)?;
    Ok(())
}

// ─── 헬퍼: 비밀 정보가 담긴 파일 쓰기 ─────────────────────────────────────

/// 소유자만 읽을 수 있는 권한(0600)으로 파일을 쓴다.
///
/// Ansible 인벤토리에는 `ansible_ssh_pass` / `ansible_become_pass` 가 평문으로
/// 들어간다. `std::fs::write` 는 umask 를 따르므로 보통 0644 로 만들어져
/// 같은 호스트의 다른 사용자가 읽을 수 있다. temp_dir 기본값이 /tmp/icm 라
/// 더 위험하다.
pub(crate) fn write_private(path: &str, content: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // 기존 파일이 느슨한 권한으로 남아 있을 수 있으므로 지우고 새로 만든다
    // (OpenOptions 의 mode 는 **새로 생성될 때만** 적용된다).
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// SSH 키 경로의 선행 `~` 를 홈 디렉터리로 바꾼다.
///
/// `str::replace` 는 경로 **중간의** `~` 까지 바꿔버린다
/// (예: `/keys/my~key` → `/keys/my/home/userkey`). 선행 `~/` 와 단독 `~` 만 처리한다.
fn expand_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return path.to_string();
    }
    if path == "~" {
        home
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        path.to_string()
    }
}

// ─── 헬퍼: Ansible 인벤토리 생성 ─────────────────────────────────────────

fn write_ansible_inventory(
    nodes:   &[DbNode],
    profile: &Option<DbAnsibleProfile>,
    path:    &str,
) -> anyhow::Result<()> {
    let mut lines = vec!["[all]".to_string()];
    for node in nodes {
        let ssh_user = profile.as_ref()
            .map(|p| p.ssh_user.as_str())
            .unwrap_or(node.ssh_user.as_str());
        let mut line = format!(
            "{} ansible_host={} ansible_user={}",
            node.hostname,
            if node.ip.is_empty() { &node.hostname } else { &node.ip },
            ssh_user,
        );
        if let Some(ref p) = profile {
            match p.auth_method.as_str() {
                "key" if !p.ssh_key.is_empty() =>
                    line.push_str(&format!(" ansible_ssh_private_key_file={}", p.ssh_key)),
                "password" if !p.ssh_password.is_empty() =>
                    line.push_str(&format!(" ansible_ssh_pass={}", p.ssh_password)),
                _ => {}
            }
            if p.do_become {
                line.push_str(&format!(" ansible_become=yes ansible_become_method={}", p.become_method));
                if !p.become_password.is_empty() {
                    line.push_str(&format!(" ansible_become_pass={}", p.become_password));
                }
            }
        }
        lines.push(line);
    }
    lines.push("\n[all:vars]".to_string());
    lines.push("ansible_host_key_checking=False".to_string());
    // 인벤토리에는 ansible_ssh_pass / ansible_become_pass 가 평문으로 들어간다
    write_private(path, &lines.join("\n"))?;
    Ok(())
}

// ─── 헬퍼: 수집 플레이북 생성 ─────────────────────────────────────────────

fn write_collect_playbook(temp_dir: &str, path: &str) -> anyhow::Result<()> {
    let content = format!(r#"---
- name: Collect network interface information from the nodes
  hosts: all
  gather_facts: no
  tasks:
    - name: Collect IP address information (JSON)
      command: ip -j addr show
      register: ip_json
      changed_when: false

    - name: Save the collected data locally
      local_action:
        module: copy
        content: "{{{{ ip_json.stdout }}}}"
        dest: "{temp_dir}/ifaces_{{{{ inventory_hostname }}}}.json"
        mode: '0600'
"#, temp_dir = temp_dir);
    std::fs::write(path, content)?;
    Ok(())
}

// ─── 헬퍼: `ip -j addr show` JSON 파싱 ───────────────────────────────────

/// 로컬백/가상 인터페이스 제외 패턴
fn is_virtual_iface(name: &str) -> bool {
    let skip = ["lo", "virbr", "docker", "br-", "veth", "dummy", "tunl", "sit"];
    skip.iter().any(|p| name.starts_with(p))
}

fn parse_ip_addr_json(hostname: &str, json: &str) -> anyhow::Result<Vec<DbNodeInterface>> {
    let data: serde_json::Value = serde_json::from_str(json)?;
    let arr = data.as_array()
        .ok_or_else(|| anyhow::anyhow!("not a JSON array"))?;

    let mut result = Vec::new();
    for iface in arr {
        let ifname = iface["ifname"].as_str().unwrap_or("").to_string();
        if ifname.is_empty() || is_virtual_iface(&ifname) { continue; }

        if let Some(addr_info) = iface["addr_info"].as_array() {
            for addr in addr_info {
                let family     = addr["family"].as_str().unwrap_or("");
                let ip         = addr["local"].as_str().unwrap_or("").to_string();
                let prefix_len = addr["prefixlen"].as_u64().unwrap_or(0) as u8;
                if (family == "inet" || family == "inet6") && !ip.is_empty() {
                    // link-local IPv6 제외 (fe80::)
                    if family == "inet6" && ip.starts_with("fe80") { continue; }
                    result.push(DbNodeInterface {
                        id:         0,
                        hostname:   hostname.to_string(),
                        interface:  ifname.clone(),
                        ip,
                        prefix_len,
                        family:     family.to_string(),
                        created_at: String::new(),
                    });
                }
            }
        }
    }
    Ok(result)
}

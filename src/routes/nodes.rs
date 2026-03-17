use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Json},
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::db::{
    DbNode, DbNetwork, DbAnsibleProfile,
    list_nodes, upsert_node, delete_node,
    list_networks, upsert_network, delete_network,
    list_ansible_profiles, upsert_ansible_profile, delete_ansible_profile,
};
use crate::scan;

// ─── 페이지 ────────────────────────────────────────────────────────────────

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("nodes/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─── 재스캔 ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ScanResult {
    pub nodes_added: usize,
    pub networks_added: usize,
}

pub async fn rescan(State(state): State<AppState>) -> Result<Json<ScanResult>, StatusCode> {
    // icm.conf 의 경로 설정을 재로딩하지 않고, AppState에 없으므로
    // 기본 경로만 사용. 실제 프로젝트에선 AppState에 설정을 포함시키면 됨.
    let drbd_nodes = scan::scan_drbd_nodes("/etc/drbd.d");
    let networks = scan::scan_quadlet_networks("/etc/containers/systemd");

    let mut nodes_added = 0usize;
    let mut networks_added = 0usize;

    {
        let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for node in &drbd_nodes {
            if upsert_node(&conn, node).is_ok() { nodes_added += 1; }
        }
        for net in &networks {
            if upsert_network(&conn, net).is_ok() { networks_added += 1; }
        }
    }

    Ok(Json(ScanResult { nodes_added, networks_added }))
}

// ─── 노드 API ──────────────────────────────────────────────────────────────

pub async fn api_list_nodes(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbNode>>, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let nodes = list_nodes(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(nodes))
}

#[derive(Deserialize)]
pub struct NodeInput {
    pub hostname: String,
    pub ip: Option<String>,
    pub ssh_user: Option<String>,
}

pub async fn api_upsert_node(
    State(state): State<AppState>,
    Json(input): Json<NodeInput>,
) -> Result<StatusCode, StatusCode> {
    let node = DbNode {
        id: 0,
        hostname: input.hostname,
        ip: input.ip.unwrap_or_default(),
        ssh_user: input.ssh_user.unwrap_or_else(|| "root".to_string()),
        source: "manual".to_string(),
        created_at: String::new(),
    };
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    upsert_node(&conn, &node).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn api_delete_node(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    delete_node(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

// ─── 네트워크 API ──────────────────────────────────────────────────────────

pub async fn api_list_networks(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbNetwork>>, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let nets = list_networks(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(nets))
}

#[derive(Deserialize)]
pub struct NetworkInput {
    pub name: String,
    pub driver: Option<String>,
    pub interface: Option<String>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub ipvlan_mode: Option<String>,
}

pub async fn api_upsert_network(
    State(state): State<AppState>,
    Json(input): Json<NetworkInput>,
) -> Result<StatusCode, StatusCode> {
    let net = DbNetwork {
        id: 0,
        name: input.name,
        driver: input.driver.unwrap_or_else(|| "ipvlan".to_string()),
        interface: input.interface.unwrap_or_default(),
        subnet: input.subnet.unwrap_or_default(),
        gateway: input.gateway.unwrap_or_default(),
        ipvlan_mode: input.ipvlan_mode.unwrap_or_else(|| "l2".to_string()),
        source: "manual".to_string(),
        created_at: String::new(),
    };
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    upsert_network(&conn, &net).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn api_delete_network(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    delete_network(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

// ─── Ansible 프로파일 API ──────────────────────────────────────────────────

pub async fn api_list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<DbAnsibleProfile>>, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let profiles = list_ansible_profiles(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(profiles))
}

#[derive(Deserialize)]
pub struct AnsibleProfileInput {
    pub name: String,
    pub auth_method: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_key: Option<String>,
    pub ssh_password: Option<String>,
    #[serde(rename = "become")]
    pub do_become: Option<bool>,
    pub become_method: Option<String>,
    pub become_password: Option<String>,
}

pub async fn api_upsert_profile(
    State(state): State<AppState>,
    Json(input): Json<AnsibleProfileInput>,
) -> Result<StatusCode, StatusCode> {
    let profile = DbAnsibleProfile {
        id: 0,
        name: input.name,
        auth_method: input.auth_method.unwrap_or_else(|| "key".to_string()),
        ssh_user: input.ssh_user.unwrap_or_else(|| "root".to_string()),
        ssh_key: input.ssh_key.unwrap_or_else(|| "~/.ssh/id_rsa".to_string()),
        ssh_password: input.ssh_password.unwrap_or_default(),
        do_become: input.do_become.unwrap_or(true),
        become_method: input.become_method.unwrap_or_else(|| "sudo".to_string()),
        become_password: input.become_password.unwrap_or_default(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    upsert_ansible_profile(&conn, &profile).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn api_delete_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let conn = state.db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    delete_ansible_profile(&conn, id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

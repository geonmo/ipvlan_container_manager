use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::db::{self, DbVolume};
use crate::generators::quadlet::generate_bind_volume_unit;

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let mut ctx = Context::new();
    let conn = state.db.lock().unwrap();
    let volumes = db::list_volumes(&conn).unwrap_or_default();
    drop(conn);
    ctx.insert("volumes", &volumes);
    let rendered = state.tera.render("volume/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ── API ─────────────────────────────────────────────────────────────────────

pub async fn api_list(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::list_volumes(&conn) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_upsert(
    State(state): State<AppState>,
    Json(body): Json<DbVolume>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::upsert_volume(&conn, &body) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::delete_volume(&conn, id) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── DRBD 리소스 API (자동 가져오기용) ─────────────────────────────────────────

pub async fn api_list_drbd_resources(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::list_resources(&conn) {
        Ok(resources) => {
            let items: Vec<serde_json::Value> = resources.iter().map(|r| {
                // nodes_json에서 lvm_vg 추출 (첫 번째 LVM 노드)
                let nodes: Vec<serde_json::Value> = serde_json::from_str(&r.nodes_json).unwrap_or_default();
                let lvm_vg = nodes.iter()
                    .find(|n| n["disk_type"].as_str() == Some("lvm"))
                    .and_then(|n| n["lvm_vg"].as_str())
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({
                    "name": r.resource_name,
                    "minor": r.minor,
                    "lvm_vg": lvm_vg,
                })
            }).collect();
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Generate ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct VolumeGenerateRequest {
    pub volume_names: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VolumeResult {
    pub files: Vec<(String, String)>,
    pub ansible_playbook: String,
}

pub async fn generate(
    State(state): State<AppState>,
    Json(body): Json<VolumeGenerateRequest>,
) -> Html<String> {
    let mut ctx = Context::new();

    let volumes = {
        let conn = state.db.lock().unwrap();
        db::list_volumes(&conn).unwrap_or_default()
    };

    // DRBD 리소스와 연결된 볼륨은 Pacemaker가 관리하므로 Quadlet 파일 생성 제외
    let selected: Vec<&DbVolume> = if body.volume_names.is_empty() {
        volumes.iter().filter(|v| v.drbd_resource.is_empty()).collect()
    } else {
        volumes.iter()
            .filter(|v| body.volume_names.contains(&v.name) && v.drbd_resource.is_empty())
            .collect()
    };

    if selected.is_empty() {
        ctx.insert("error", "생성할 볼륨이 없습니다.");
        ctx.insert("result", &serde_json::Value::Null);
        let rendered = state.tera.render("volume/result.html", &ctx)
            .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
        return Html(rendered);
    }

    let files: Vec<(String, String)> = selected.iter().map(|v| {
        let filename = format!("{}.volume", v.name);
        let content = generate_bind_volume_unit(&v.name, &v.host_path, &v.description);
        (filename, content)
    }).collect();

    let ansible_playbook = build_volume_ansible_playbook(&files);

    let result = VolumeResult { files, ansible_playbook };
    ctx.insert("result", &result);
    ctx.insert("error", &serde_json::Value::Null);

    let rendered = state.tera.render("volume/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

fn build_volume_ansible_playbook(files: &[(String, String)]) -> String {
    let install_path = "/etc/containers/systemd";
    let mut tasks = vec![
        format!("    - name: Quadlet 디렉토리 생성\n      file:\n        path: {}\n        state: directory\n        mode: '0755'", install_path),
    ];
    for (filename, content) in files {
        let indented = content.lines()
            .map(|l| format!("          {}", l))
            .collect::<Vec<_>>()
            .join("\n");
        tasks.push(format!(
            "    - name: {} 배포\n      copy:\n        dest: {}/{}\n        content: |\n{}\n        mode: '0644'",
            filename, install_path, filename, indented
        ));
    }
    tasks.push("    - name: systemd 데몬 리로드\n      systemd:\n        daemon_reload: yes".to_string());

    format!(
        "---\n- name: Quadlet Volume 유닛 배포\n  hosts: all\n  become: yes\n  tasks:\n{}",
        tasks.join("\n\n")
    )
}

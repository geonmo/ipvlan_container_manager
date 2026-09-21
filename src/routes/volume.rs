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
//
// 스토리지 백엔드에 따라 출처가 갈린다:
//   kernel-module : `.res` 파일 스캔 결과가 담긴 drbd_resources 테이블
//   linstor       : LINSTOR 컨트롤러 REST API (리소스를 LINSTOR가 만들기 때문에
//                   `.res` 파일이 /var/lib/linstor.d/ 아래에 생겨 이 앱의
//                   /etc/drbd.d 스캔에는 잡히지 않는다)
//
// 응답은 `{backend, resources, warning}` 객체다. 예전에는 배열만 돌려줬는데,
// 그러면 LINSTOR 컨트롤러에 못 붙었을 때 UI가 "리소스 없음"과 구분하지 못하고
// 조용히 빈 드롭다운을 보여준다. warning으로 이유를 전달한다.

/// LINSTOR가 아는 리소스 목록을 REST API에서 가져온다.
///
/// `/v1/view/resources`는 노드별 배치가 한 항목씩 오므로 리소스 이름으로 묶는다.
/// minor 번호는 `volumes[].device_path`(예: /dev/drbd1000)에서 뽑는다 —
/// LINSTOR가 minor를 직접 필드로 주지 않는다.
async fn fetch_linstor_resources(base_url: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let url = format!("{}/v1/view/resources", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("LINSTOR API {} 응답: HTTP {}", url, resp.status());
    }
    let rows: Vec<serde_json::Value> = resp.json().await?;
    Ok(aggregate_linstor_rows(&rows))
}

/// `/v1/view/resources` 의 노드별 행을 리소스 단위로 묶는다.
///
/// HTTP와 분리된 순수 함수로 둬서 응답 파싱 규칙을 테스트로 고정한다.
fn aggregate_linstor_rows(rows: &[serde_json::Value]) -> Vec<serde_json::Value> {
    // 리소스 이름 → (노드 목록, minor, 스토리지 풀). order 로 입력 순서를 보존한다.
    let mut order: Vec<String> = Vec::new();
    let mut acc: std::collections::HashMap<String, (Vec<String>, Option<u32>, String)> =
        std::collections::HashMap::new();

    for row in rows {
        let Some(name) = row["name"].as_str() else { continue };
        let node = row["node_name"].as_str().unwrap_or("").to_string();

        let entry = acc.entry(name.to_string()).or_insert_with(|| {
            order.push(name.to_string());
            (Vec::new(), None, String::new())
        });
        if !node.is_empty() && !entry.0.contains(&node) {
            entry.0.push(node);
        }

        if let Some(vols) = row["volumes"].as_array() {
            for v in vols {
                if entry.1.is_none() {
                    entry.1 = v["device_path"]
                        .as_str()
                        .and_then(|p| p.rsplit_once("/dev/drbd").map(|(_, n)| n.to_string()))
                        .and_then(|n| n.parse::<u32>().ok());
                }
                // 디스크리스 배치에는 스토리지 풀이 의미가 없으므로 건너뛴다
                // (DfltDisklessStorPool 이 UI 라벨로 새어 나가면 안 된다).
                if entry.2.is_empty() && v["provider_kind"].as_str() != Some("DISKLESS") {
                    if let Some(sp) = v["storage_pool_name"].as_str() {
                        entry.2 = sp.to_string();
                    }
                }
            }
        }
    }

    order
        .into_iter()
        .map(|name| {
            let (nodes, minor, pool) = acc.remove(&name).unwrap_or_default();
            serde_json::json!({
                "name": name,
                "minor": minor,
                "lvm_vg": "",            // LINSTOR 경로에서는 VG 대신 스토리지 풀을 쓴다
                "storage_pool": pool,
                "nodes": nodes,
                "source": "linstor",
            })
        })
        .collect()
}

/// `.res` 파일 스캔 결과(DB)에서 리소스 목록을 만든다.
fn db_drbd_resources(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<serde_json::Value>> {
    Ok(db::list_resources(conn)?
        .iter()
        .map(|r| {
            // nodes_json에서 lvm_vg 추출 (첫 번째 LVM 노드)
            let nodes: Vec<serde_json::Value> =
                serde_json::from_str(&r.nodes_json).unwrap_or_default();
            let lvm_vg = nodes
                .iter()
                .find(|n| n["disk_type"].as_str() == Some("lvm"))
                .and_then(|n| n["lvm_vg"].as_str())
                .unwrap_or("")
                .to_string();
            let node_names: Vec<String> = nodes
                .iter()
                .filter_map(|n| n["hostname"].as_str().map(str::to_string))
                .collect();
            serde_json::json!({
                "name": r.resource_name,
                "minor": r.minor,
                "lvm_vg": lvm_vg,
                "storage_pool": "",
                "nodes": node_names,
                "source": "file",
            })
        })
        .collect())
}

pub async fn api_list_drbd_resources(State(state): State<AppState>) -> impl IntoResponse {
    // 백엔드 판정과 DB 조회를 먼저 끝내고 락을 놓는다 (이후 await 구간 때문).
    let (backend, db_items) = {
        let conn = state.db.lock().unwrap();
        let backend = db::get_storage_backend(&conn)
            .map(|b| b.backend)
            .unwrap_or_else(|_| "kernel-module".to_string());
        let items = db_drbd_resources(&conn);
        (backend, items)
    };

    let db_items = match db_items {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if backend != "linstor" {
        return Json(serde_json::json!({
            "backend": backend,
            "resources": db_items,
            "warning": serde_json::Value::Null,
        }))
        .into_response();
    }

    match fetch_linstor_resources(&state.linstor_url).await {
        Ok(items) => Json(serde_json::json!({
            "backend": "linstor",
            "resources": items,
            "warning": serde_json::Value::Null,
        }))
        .into_response(),
        Err(e) => {
            // 컨트롤러에 못 붙어도 200으로 응답하되 이유를 남긴다. 스캔으로
            // 들어온 .res 기반 목록이 있으면 그거라도 보여준다.
            tracing::warn!("LINSTOR 리소스 조회 실패({}): {}", state.linstor_url, e);
            Json(serde_json::json!({
                "backend": "linstor",
                "resources": db_items,
                "warning": format!(
                    "LINSTOR 컨트롤러({})에 연결하지 못했습니다: {}.                      아래 목록은 /etc/drbd.d 스캔 결과이며 LINSTOR가 관리하는                      리소스가 빠져 있을 수 있습니다.",
                    state.linstor_url, e
                ),
            }))
            .into_response()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// LINSTOR `/v1/view/resources` 는 **노드별 배치가 한 항목씩** 온다.
    /// 리소스 이름으로 묶어야 하고, minor 는 device_path 에서 뽑아야 한다
    /// (LINSTOR 가 minor 를 별도 필드로 주지 않는다).
    fn sample_view_resources() -> serde_json::Value {
        serde_json::json!([
            {
                "name": "res-icmtest",
                "node_name": "node01.build.test",
                "volumes": [{
                    "volume_number": 0,
                    "storage_pool_name": "pool-ssd",
                    "provider_kind": "LVM_THIN",
                    "device_path": "/dev/drbd1000"
                }]
            },
            {
                "name": "res-icmtest",
                "node_name": "node02.build.test",
                "volumes": [{
                    "volume_number": 0,
                    "storage_pool_name": "pool-ssd",
                    "provider_kind": "LVM_THIN",
                    "device_path": "/dev/drbd1000"
                }]
            },
            {
                // 디스크리스 배치 — 스토리지 풀 이름을 가져오면 안 된다
                "name": "res-icmtest",
                "node_name": "node03.build.test",
                "volumes": [{
                    "volume_number": 0,
                    "storage_pool_name": "DfltDisklessStorPool",
                    "provider_kind": "DISKLESS",
                    "device_path": "/dev/drbd1000"
                }]
            }
        ])
    }

    #[test]
    fn linstor_rows_are_grouped_by_resource_name() {
        let rows = sample_view_resources();
        let out = aggregate_linstor_rows(rows.as_array().unwrap());

        // 노드별 3행이 리소스 1건으로 묶여야 한다
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "res-icmtest");
        assert_eq!(
            out[0]["nodes"].as_array().unwrap().len(),
            3,
            "배치된 3개 노드가 모두 잡혀야 한다"
        );
    }

    #[test]
    fn minor_is_parsed_from_device_path() {
        let rows = sample_view_resources();
        let out = aggregate_linstor_rows(rows.as_array().unwrap());
        assert_eq!(out[0]["minor"], 1000, "/dev/drbd1000 -> 1000");
    }

    #[test]
    fn diskless_placement_does_not_supply_the_storage_pool() {
        let rows = sample_view_resources();
        let out = aggregate_linstor_rows(rows.as_array().unwrap());
        assert_eq!(
            out[0]["storage_pool"], "pool-ssd",
            "DfltDisklessStorPool 이 아니라 실제 디스크풀 이름이어야 한다"
        );
    }

    /// 디스크리스만 있는 리소스는 스토리지 풀이 빈 문자열이어야 한다
    /// (UI 라벨이 'DfltDisklessStorPool' 로 오염되면 안 된다).
    #[test]
    fn diskless_only_resource_has_empty_storage_pool() {
        let rows = serde_json::json!([{
            "name": "res-diskless",
            "node_name": "node01.build.test",
            "volumes": [{
                "storage_pool_name": "DfltDisklessStorPool",
                "provider_kind": "DISKLESS",
                "device_path": "/dev/drbd1010"
            }]
        }]);
        let out = aggregate_linstor_rows(rows.as_array().unwrap());
        assert_eq!(out[0]["storage_pool"], "");
        assert_eq!(out[0]["minor"], 1010);
    }
}

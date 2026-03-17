use axum::{
    extract::{Path, State},
    response::{Html, Json},
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::db::{self, DbDrbdResource};
use crate::models::drbd::{
    AnsibleInventory, AnsibleNode, DrbdDiskOptions, DrbdNetOptions, DrbdNode, DrbdResource,
    DrbdStartupOptions,
};
use crate::generators::drbd::{
    generate_res_file, generate_ansible_inventory, generate_ansible_playbook,
    generate_drbd_init_commands, scan_drbd_dir,
};

const DRBD_DIR: &str = "/etc/drbd.d";

// ─────────────────────────────────────────────────────────────
// 스캔 응답 구조체
// ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ScanResponse {
    /// 파일 + DB 리소스 목록
    pub resources: Vec<ScanEntry>,
    /// 사용되지 않는 다음 minor 번호
    pub next_minor: u32,
    /// 사용되지 않는 다음 포트 번호
    pub next_port: u16,
}

#[derive(Serialize)]
pub struct ScanEntry {
    /// "file" | "db"
    pub source: String,
    pub resource_name: String,
    pub protocol: String,
    pub minor: u32,
    pub nodes: serde_json::Value,
    /// 파일 스캔일 때 .res 파일명
    pub source_file: String,
    /// DB 저장 시각 (DB 엔트리만)
    pub updated_at: String,
    /// DB에도 동일 이름이 저장되어 있는지 여부
    pub in_db: bool,
    // 고급 옵션 (DB 엔트리에만 있음, 폼 복원용)
    pub net_options_json: String,
    pub disk_options_json: String,
    pub startup_options_json: String,
}

// ─────────────────────────────────────────────────────────────
// GET /drbd/
// ─────────────────────────────────────────────────────────────

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("drbd/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─────────────────────────────────────────────────────────────
// GET /drbd/scan
// ─────────────────────────────────────────────────────────────

pub async fn scan(State(state): State<AppState>) -> Json<serde_json::Value> {
    // 1) /etc/drbd.d 파일 스캔
    let file_resources = scan_drbd_dir(DRBD_DIR);

    // 2) DB 조회
    let db_resources = {
        let conn = state.db.lock().unwrap();
        db::list_resources(&conn).unwrap_or_default()
    };

    // DB 이름 셋 (중복 체크용)
    let db_names: std::collections::HashSet<String> =
        db_resources.iter().map(|r| r.resource_name.clone()).collect();
    let file_names: std::collections::HashSet<String> =
        file_resources.iter().map(|r| r.resource_name.clone()).collect();

    // 3) 사용 중인 minor / port 수집
    let mut used_minors: Vec<u32> = Vec::new();
    let mut used_ports:  Vec<u16> = Vec::new();

    for r in &file_resources {
        used_minors.push(r.minor);
        for n in &r.nodes { used_ports.push(n.port); }
    }
    for r in &db_resources {
        used_minors.push(r.minor);
        if let Ok(nodes) = serde_json::from_str::<Vec<serde_json::Value>>(&r.nodes_json) {
            for n in &nodes {
                if let Some(p) = n.get("port").and_then(|v| v.as_u64()) {
                    used_ports.push(p as u16);
                }
            }
        }
    }
    used_minors.sort_unstable();
    used_minors.dedup();
    used_ports.sort_unstable();
    used_ports.dedup();

    let next_minor = next_available_u32(&used_minors, 0);
    let next_port  = next_available_u16(&used_ports, 7789);

    // 4) 응답 목록 구성
    let mut entries: Vec<ScanEntry> = Vec::new();

    // 파일 리소스
    for r in &file_resources {
        entries.push(ScanEntry {
            source:               "file".to_string(),
            resource_name:        r.resource_name.clone(),
            protocol:             r.protocol.clone(),
            minor:                r.minor,
            nodes:                serde_json::to_value(&r.nodes).unwrap_or_default(),
            source_file:          r.source_file.clone(),
            updated_at:           String::new(),
            in_db:                db_names.contains(&r.resource_name),
            net_options_json:     String::new(),
            disk_options_json:    String::new(),
            startup_options_json: String::new(),
        });
    }

    // DB 전용 리소스 (파일에 없는 것만)
    for r in &db_resources {
        if file_names.contains(&r.resource_name) {
            continue; // 이미 파일 목록에 있음
        }
        entries.push(ScanEntry {
            source:               "db".to_string(),
            resource_name:        r.resource_name.clone(),
            protocol:             r.protocol.clone(),
            minor:                r.minor,
            nodes:                serde_json::from_str(&r.nodes_json).unwrap_or_default(),
            source_file:          String::new(),
            updated_at:           r.updated_at.clone(),
            in_db:                true,
            net_options_json:     r.net_options_json.clone(),
            disk_options_json:    r.disk_options_json.clone(),
            startup_options_json: r.startup_options_json.clone(),
        });
    }

    // DB에만 있는 항목에 updated_at 채우기 (파일 항목도 in_db이면 표시)
    for entry in entries.iter_mut() {
        if entry.in_db && entry.updated_at.is_empty() {
            if let Some(dbr) = db_resources.iter().find(|r| r.resource_name == entry.resource_name) {
                entry.updated_at           = dbr.updated_at.clone();
                entry.net_options_json     = dbr.net_options_json.clone();
                entry.disk_options_json    = dbr.disk_options_json.clone();
                entry.startup_options_json = dbr.startup_options_json.clone();
            }
        }
    }

    // minor 오름차순 정렬
    entries.sort_by_key(|e| e.minor);

    Json(serde_json::json!({
        "resources": entries,
        "next_minor": next_minor,
        "next_port":  next_port,
    }))
}

fn next_available_u32(used: &[u32], base: u32) -> u32 {
    let mut candidate = base;
    for &v in used {
        if v == candidate { candidate += 1; }
    }
    candidate
}

fn next_available_u16(used: &[u16], base: u16) -> u16 {
    let mut candidate = base;
    for &v in used {
        if v == candidate { candidate += 1; }
    }
    candidate
}

// ─────────────────────────────────────────────────────────────
// Form 구조체
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DrbdFormData {
    pub resource_name: String,
    pub protocol: String,
    pub minor: u32,
    /// JSON 배열: [{hostname, ip, disk_type, disk, lvm_vg, lvm_size, meta, port}, ...]
    pub nodes_json: String,
    // net options
    pub allow_two_primaries: Option<String>,
    pub after_sb_0pri: String,
    pub after_sb_1pri: String,
    pub after_sb_2pri: String,
    // disk options
    pub on_io_error: String,
    pub fencing: String,
    // startup
    pub wfc_timeout: u32,
    pub degr_wfc_timeout: u32,
    pub become_primary_on: Option<String>,
    // ansible
    pub ansible_user: String,
    pub ansible_ssh_key: String,
    pub ansible_become: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NodeInput {
    hostname: String,
    ip: String,
    #[serde(default = "default_disk_type")]
    disk_type: String,
    #[serde(default)]
    disk: String,
    #[serde(default)]
    lvm_vg: String,
    #[serde(default)]
    lvm_size: String,
    #[serde(default = "default_meta")]
    meta: String,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_disk_type() -> String { "block".to_string() }
fn default_meta()      -> String { "internal".to_string() }
fn default_port()      -> u16    { 7789 }

fn parse_nodes(nodes_json: &str) -> Vec<DrbdNode> {
    let inputs: Vec<NodeInput> = serde_json::from_str(nodes_json).unwrap_or_default();
    inputs.into_iter()
        .filter(|n| !n.hostname.is_empty() && !n.ip.is_empty())
        .filter(|n| {
            if n.disk_type == "lvm" { !n.lvm_vg.is_empty() }
            else { !n.disk.is_empty() }
        })
        .map(|n| DrbdNode {
            hostname:    n.hostname,
            ip:          n.ip,
            disk_device: n.disk,
            meta_disk:   if n.meta.is_empty() { "internal".to_string() } else { n.meta },
            port:        if n.port == 0 { 7789 } else { n.port },
            disk_type:   n.disk_type,
            lvm_vg:      n.lvm_vg,
            lvm_size:    n.lvm_size,
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────
// 결과 구조체
// ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DrbdResult {
    pub res_file: String,
    pub inventory_yaml: String,
    pub ansible_playbook: String,
    pub init_commands: Vec<String>,
    pub res_filename: String,
    pub has_lvm: bool,
    /// DB 저장 폼 복원용 원본 데이터
    pub save_form: SaveForm,
}

#[derive(Serialize)]
pub struct SaveForm {
    pub resource_name: String,
    pub protocol: String,
    pub minor: u32,
    pub nodes_json: String,
    pub allow_two_primaries: bool,
    pub after_sb_0pri: String,
    pub after_sb_1pri: String,
    pub after_sb_2pri: String,
    pub on_io_error: String,
    pub fencing: String,
    pub wfc_timeout: u32,
    pub degr_wfc_timeout: u32,
    pub become_primary_on: String,
    pub ansible_user: String,
    pub ansible_ssh_key: String,
    pub ansible_become: bool,
}

// ─────────────────────────────────────────────────────────────
// POST /drbd/generate
// ─────────────────────────────────────────────────────────────

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<DrbdFormData>,
) -> Html<String> {
    let nodes = parse_nodes(&form.nodes_json);

    if nodes.len() < 2 {
        let mut ctx = Context::new();
        ctx.insert("error", "노드를 최소 2개 이상 입력해야 합니다.");
        let rendered = state.tera.render("drbd/result.html", &ctx)
            .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
        return Html(rendered);
    }

    let resource  = build_resource(&form, nodes.clone());
    let inventory = build_inventory(&form, &nodes);
    let has_lvm   = nodes.iter().any(|n| n.disk_type == "lvm");

    let res_file         = generate_res_file(&resource);
    let inventory_yaml   = generate_ansible_inventory(&inventory)
        .unwrap_or_else(|e| format!("# Error: {}", e));
    let ansible_playbook = generate_ansible_playbook(&resource, &inventory);
    let init_commands    = generate_drbd_init_commands(&resource);
    let res_filename     = format!("{}.res", resource.resource_name);

    let save_form = SaveForm {
        resource_name:       form.resource_name.clone(),
        protocol:            form.protocol.clone(),
        minor:               form.minor,
        nodes_json:          form.nodes_json.clone(),
        allow_two_primaries: form.allow_two_primaries.as_deref() == Some("on"),
        after_sb_0pri:       form.after_sb_0pri.clone(),
        after_sb_1pri:       form.after_sb_1pri.clone(),
        after_sb_2pri:       form.after_sb_2pri.clone(),
        on_io_error:         form.on_io_error.clone(),
        fencing:             form.fencing.clone(),
        wfc_timeout:         form.wfc_timeout,
        degr_wfc_timeout:    form.degr_wfc_timeout,
        become_primary_on:   form.become_primary_on.clone().unwrap_or_default(),
        ansible_user:        form.ansible_user.clone(),
        ansible_ssh_key:     form.ansible_ssh_key.clone(),
        ansible_become:      form.ansible_become.as_deref() == Some("on"),
    };

    let result = DrbdResult {
        res_file,
        inventory_yaml,
        ansible_playbook,
        init_commands,
        res_filename,
        has_lvm,
        save_form,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);

    let rendered = state.tera.render("drbd/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─────────────────────────────────────────────────────────────
// POST /drbd/save  — DB에 저장
// ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SaveFormData {
    pub resource_name:       String,
    pub protocol:            String,
    pub minor:               u32,
    pub nodes_json:          String,
    pub allow_two_primaries: Option<String>,
    pub after_sb_0pri:       Option<String>,
    pub after_sb_1pri:       Option<String>,
    pub after_sb_2pri:       Option<String>,
    pub on_io_error:         Option<String>,
    pub fencing:             Option<String>,
    pub wfc_timeout:         Option<u32>,
    pub degr_wfc_timeout:    Option<u32>,
    pub become_primary_on:   Option<String>,
    pub source:              Option<String>,
}

pub async fn save(
    State(state): State<AppState>,
    Form(form): Form<SaveFormData>,
) -> Json<serde_json::Value> {
    let net = serde_json::json!({
        "allow_two_primaries": form.allow_two_primaries.as_deref() == Some("on"),
        "after_sb_0pri": form.after_sb_0pri.as_deref().unwrap_or("discard-younger-primary"),
        "after_sb_1pri": form.after_sb_1pri.as_deref().unwrap_or("discard-secondary"),
        "after_sb_2pri": form.after_sb_2pri.as_deref().unwrap_or("disconnect"),
    });
    let disk = serde_json::json!({
        "on_io_error": form.on_io_error.as_deref().unwrap_or("passthrough"),
        "fencing":     form.fencing.as_deref().unwrap_or("resource-only"),
    });
    let startup = serde_json::json!({
        "wfc_timeout":       form.wfc_timeout.unwrap_or(15),
        "degr_wfc_timeout":  form.degr_wfc_timeout.unwrap_or(60),
        "become_primary_on": form.become_primary_on.as_deref().unwrap_or(""),
    });

    let record = DbDrbdResource {
        id:                   0,
        resource_name:        form.resource_name.clone(),
        protocol:             form.protocol.clone(),
        minor:                form.minor,
        nodes_json:           form.nodes_json.clone(),
        net_options_json:     serde_json::to_string(&net).unwrap_or_default(),
        disk_options_json:    serde_json::to_string(&disk).unwrap_or_default(),
        startup_options_json: serde_json::to_string(&startup).unwrap_or_default(),
        source:               form.source.as_deref().unwrap_or("manual").to_string(),
        created_at:           String::new(),
        updated_at:           String::new(),
    };

    let result = {
        let conn = state.db.lock().unwrap();
        db::upsert_resource(&conn, &record)
    };

    match result {
        Ok(_) => Json(serde_json::json!({
            "ok": true,
            "message": format!("'{}' 저장 완료", form.resource_name)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("저장 실패: {}", e)
        })),
    }
}

// ─────────────────────────────────────────────────────────────
// DELETE /drbd/delete/:name  — DB에서 삭제
// ─────────────────────────────────────────────────────────────

pub async fn delete_saved(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    let result = {
        let conn = state.db.lock().unwrap();
        db::delete_resource(&conn, &name)
    };
    match result {
        Ok(n) if n > 0 => Json(serde_json::json!({ "ok": true, "message": format!("'{}' 삭제됨", name) })),
        Ok(_)          => Json(serde_json::json!({ "ok": false, "message": "해당 리소스 없음" })),
        Err(e)         => Json(serde_json::json!({ "ok": false, "message": format!("삭제 실패: {}", e) })),
    }
}

// ─────────────────────────────────────────────────────────────
// POST /drbd/download
// ─────────────────────────────────────────────────────────────

pub async fn download_res(
    State(_state): State<AppState>,
    Form(form): Form<DrbdFormData>,
) -> axum::response::Response<String> {
    let nodes    = parse_nodes(&form.nodes_json);
    let resource = build_resource(&form, nodes);
    let content  = generate_res_file(&resource);
    let filename = format!("{}.res", resource.resource_name);

    axum::response::Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
        .body(content)
        .unwrap()
}

// ─────────────────────────────────────────────────────────────
// 헬퍼
// ─────────────────────────────────────────────────────────────

fn build_resource(form: &DrbdFormData, nodes: Vec<DrbdNode>) -> DrbdResource {
    DrbdResource {
        resource_name: form.resource_name.clone(),
        protocol:      form.protocol.clone(),
        minor:         form.minor,
        nodes,
        net_options: DrbdNetOptions {
            allow_two_primaries: form.allow_two_primaries.as_deref() == Some("on"),
            after_sb_0pri:       form.after_sb_0pri.clone(),
            after_sb_1pri:       form.after_sb_1pri.clone(),
            after_sb_2pri:       form.after_sb_2pri.clone(),
        },
        disk_options: DrbdDiskOptions {
            on_io_error: form.on_io_error.clone(),
            fencing:     form.fencing.clone(),
        },
        startup_options: DrbdStartupOptions {
            wfc_timeout:       form.wfc_timeout,
            degr_wfc_timeout:  form.degr_wfc_timeout,
            become_primary_on: form.become_primary_on.clone().unwrap_or_default(),
        },
    }
}

fn build_inventory(form: &DrbdFormData, nodes: &[DrbdNode]) -> AnsibleInventory {
    AnsibleInventory {
        nodes: nodes.iter().map(|n| AnsibleNode {
            hostname: n.hostname.clone(),
            ip:       n.ip.clone(),
        }).collect(),
        ansible_user:                    form.ansible_user.clone(),
        ansible_ssh_private_key_file:    form.ansible_ssh_key.clone(),
        r#become:                        form.ansible_become.as_deref() == Some("on"),
    }
}

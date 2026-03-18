use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Form, Json,
};
use serde::Deserialize;
use tera::Context;
use crate::AppState;
use crate::models::nft::{NftPolicy, NftSubnetGroup, NftServiceDef, NftTarget, NftGlobalRule};
use crate::generators::nft::generate_nft_policy;
use crate::db::{self, DbNftTarget, DbNftTargetRule, DbNftGlobalConfig};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("nft/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

/// HTML 폼 데이터 (application/x-www-form-urlencoded)
#[derive(Debug, Deserialize)]
pub struct NftGenerateForm {
    pub table_name:        Option<String>,
    pub device_name:       Option<String>,
    pub chain_name:        Option<String>,
    pub traceroute_start:  Option<u16>,
    pub traceroute_end:    Option<u16>,
    /// JavaScript가 직렬화한 NftGlobalRule JSON 배열
    pub global_rules_json: Option<String>,
    /// JavaScript가 직렬화한 NftTarget JSON 배열
    pub targets_json:      String,
}

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<NftGenerateForm>,
) -> Html<String> {
    let mut ctx = Context::new();

    // targets_json 파싱
    let mut targets: Vec<NftTarget> = match serde_json::from_str(&form.targets_json) {
        Ok(t) => t,
        Err(e) => {
            ctx.insert("error", &format!("대상 JSON 파싱 오류: {}", e));
            ctx.insert("result", &serde_json::Value::Null);
            let rendered = state.tera.render("nft/result.html", &ctx)
                .unwrap_or_else(|e2| format!("<pre>Template error: {}</pre>", e2));
            return Html(rendered);
        }
    };

    // subnet_group 빈 문자열 → None 정규화
    for target in &mut targets {
        for rule in &mut target.rules {
            if rule.subnet_group.as_deref().map(|s| s.is_empty()).unwrap_or(false) {
                rule.subnet_group = None;
            }
        }
    }

    // 참조된 서브넷 그룹 이름 수집
    let group_names: Vec<String> = {
        let mut names: Vec<String> = targets.iter()
            .flat_map(|t| t.rules.iter())
            .filter_map(|r| r.subnet_group.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };

    // 참조된 서비스 이름 수집
    let service_names: Vec<String> = {
        let mut names: Vec<String> = targets.iter()
            .flat_map(|t| t.rules.iter())
            .map(|r| r.service_name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };

    let device = form.device_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "eth0".to_string());

    let chain_name = form.chain_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("ingress_{}", device.replace('-', "_")));

    let table_name = form.table_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "filter_ingress".to_string());

    let filename = format!("{}.nft", table_name);

    // DB에서 그룹/서비스 조회
    let conn = state.db.lock().unwrap();
    let subnet_groups = match db::get_nft_subnet_groups_by_names(&conn, &group_names) {
        Ok(g) => g,
        Err(e) => {
            ctx.insert("error", &format!("서브넷 그룹 조회 오류: {}", e));
            ctx.insert("result", &serde_json::Value::Null);
            let rendered = state.tera.render("nft/result.html", &ctx)
                .unwrap_or_else(|e2| format!("<pre>Template error: {}</pre>", e2));
            return Html(rendered);
        }
    };
    let services = match db::get_nft_services_by_names(&conn, &service_names) {
        Ok(s) => s,
        Err(e) => {
            ctx.insert("error", &format!("서비스 조회 오류: {}", e));
            ctx.insert("result", &serde_json::Value::Null);
            let rendered = state.tera.render("nft/result.html", &ctx)
                .unwrap_or_else(|e2| format!("<pre>Template error: {}</pre>", e2));
            return Html(rendered);
        }
    };
    drop(conn);

    // global_rules_json 파싱 (없으면 traceroute 기본값으로 폴백)
    let global_rules: Vec<NftGlobalRule> = form.global_rules_json
        .as_deref()
        .filter(|s| !s.trim().is_empty() && s.trim() != "[]")
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| vec![NftGlobalRule {
            protocol:   "udp".to_string(),
            port_start: form.traceroute_start.unwrap_or(33434),
            port_end:   form.traceroute_end.unwrap_or(65535),
        }]);

    let policy = NftPolicy {
        filename: filename.clone(),
        table_name: table_name.clone(),
        device_name: device.clone(),
        chain_name: chain_name.clone(),
        traceroute_start: form.traceroute_start.unwrap_or(33434),
        traceroute_end:   form.traceroute_end.unwrap_or(65535),
        global_rules,
        targets: targets.clone(),
        subnet_groups,
        services,
    };

    let content = generate_nft_policy(&policy);

    // 생성 후 대상 및 전역 설정 DB 저장
    {
        let conn = state.db.lock().unwrap();
        for target in &policy.targets {
            let db_target = DbNftTarget {
                id:         0,
                name:       target.name.clone(),
                ipv4_addrs: target.ipv4_addrs.clone(),
                ipv6_addrs: target.ipv6_addrs.clone(),
                rules:      target.rules.iter().map(|r| DbNftTargetRule {
                    id:                0,
                    service_name:      r.service_name.clone(),
                    subnet_group_name: r.subnet_group.clone().unwrap_or_default(),
                }).collect(),
            };
            db::upsert_nft_target(&conn, &db_target).ok();
        }
        let global = DbNftGlobalConfig {
            table_name:       policy.table_name.clone(),
            device_name:      policy.device_name.clone(),
            chain_name:       policy.chain_name.clone(),
            traceroute_start: policy.traceroute_start,
            traceroute_end:   policy.traceroute_end,
            nft_file:         "/etc/nftables/ipvlan_l2.nft".to_string(),
            updated_at:       String::new(),
        };
        db::update_nft_global_config(&conn, &global).ok();
    }

    ctx.insert("result", &serde_json::json!({
        "filename": policy.filename,
        "content":  content,
        "policy":   policy,
    }));
    ctx.insert("error", &serde_json::Value::Null);

    let rendered = state.tera.render("nft/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ── NFT Subnet Group API ─────────────────────────────────────────────────────

pub async fn api_list_subnet_groups(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::list_nft_subnet_groups(&conn) {
        Ok(groups) => Json(groups).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_upsert_subnet_group(
    State(state): State<AppState>,
    Json(body): Json<NftSubnetGroup>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::upsert_nft_subnet_group(&conn, &body) {
        Ok(id) => Json(serde_json::json!({ "ok": true, "id": id })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_delete_subnet_group(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::delete_nft_subnet_group(&conn, id) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── NFT Service API ──────────────────────────────────────────────────────────

pub async fn api_list_services(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::list_nft_services(&conn) {
        Ok(services) => Json(services).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_upsert_service(
    State(state): State<AppState>,
    Json(body): Json<NftServiceDef>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::upsert_nft_service(&conn, &body) {
        Ok(id) => Json(serde_json::json!({ "ok": true, "id": id })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_delete_service(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::delete_nft_service(&conn, id) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── NFT Target API ───────────────────────────────────────────────────────────

pub async fn api_list_targets(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::list_nft_targets(&conn) {
        Ok(targets) => Json(targets).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_upsert_target(
    State(state): State<AppState>,
    Json(body): Json<DbNftTarget>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::upsert_nft_target(&conn, &body) {
        Ok(id) => Json(serde_json::json!({ "ok": true, "id": id })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_delete_target(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::delete_nft_target(&conn, id) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── NFT Global Config API ────────────────────────────────────────────────────

pub async fn api_get_global_config(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::get_nft_global_config(&conn) {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn api_update_global_config(
    State(state): State<AppState>,
    Json(body): Json<DbNftGlobalConfig>,
) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    match db::update_nft_global_config(&conn, &body) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── NFT Scan API ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct NftScanRequest {
    pub profile_name: Option<String>,
}

pub async fn api_scan(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let req: NftScanRequest = serde_json::from_slice(&body).unwrap_or_default();

    let (nft_file, profile) = {
        let conn = state.db.lock().unwrap();
        let nft_file = match db::get_nft_global_config(&conn) {
            Ok(cfg) => cfg.nft_file,
            Err(_)  => "/etc/nftables/ipvlan_l2.nft".to_string(),
        };
        let profile = req.profile_name.as_deref()
            .and_then(|name| db::get_ansible_profile_by_name(&conn, name).ok().flatten());
        (nft_file, profile)
    };

    match crate::nft_scanner::ensure_and_scan(&state.temp_dir, &nft_file, &state.db, profile.as_ref()).await {
        Ok(result) => Json(serde_json::json!({
            "ok":              true,
            "message":         result.message,
            "file_existed":    result.file_existed,
            "groups_updated":  result.groups_updated,
            "services_updated": result.services_updated,
            "targets_updated": result.targets_updated,
            "ansible_output":  result.ansible_output,
        })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        ).into_response(),
    }
}

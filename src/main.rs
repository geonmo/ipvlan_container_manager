mod models;
mod generators;
mod routes;
mod db;
mod scan;
mod nft_scanner;

use axum::{
    routing::{get, post, delete},
    Router,
};
use configparser::ini::Ini;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tera::{Tera, Value};
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const CONFIG_FILE: &str = "icm.conf";

#[derive(Clone)]
pub struct AppState {
    pub tera:        Arc<Tera>,
    pub db:          Arc<Mutex<rusqlite::Connection>>,
    pub temp_dir:    String,
    pub deploy_mode: String,
}

fn filter_starts_with(value: &Value, args: &HashMap<String, Value>) -> tera::Result<Value> {
    let s = value.as_str().unwrap_or("");
    let pat = args
        .get("pat")
        .and_then(|v| v.as_str())
        .ok_or_else(|| tera::Error::msg("starts_with: 'pat' 인수가 필요합니다"))?;
    Ok(Value::Bool(s.starts_with(pat)))
}

fn filter_ends_with(value: &Value, args: &HashMap<String, Value>) -> tera::Result<Value> {
    let s = value.as_str().unwrap_or("");
    let pat = args
        .get("pat")
        .and_then(|v| v.as_str())
        .ok_or_else(|| tera::Error::msg("ends_with: 'pat' 인수가 필요합니다"))?;
    Ok(Value::Bool(s.ends_with(pat)))
}

struct Config {
    host:        String,
    port:        u16,
    log_level:   String,
    db_path:     String,
    // [paths]
    drbd_dir:    String,
    quadlet_dir: String,
    temp_dir:    String,
    deploy_mode: String,
    // [pacemaker]
    pcsd_url:    String,
    pcsd_user:   String,
    pcsd_pass:   String,
}

fn load_config() -> Config {
    let mut cfg = Ini::new();

    if let Err(e) = cfg.load(CONFIG_FILE) {
        eprintln!("[WARN] {} 로드 실패: {} — 기본값 사용", CONFIG_FILE, e);
        return Config {
            host:        "0.0.0.0".to_string(),
            port:        5000,
            log_level:   "info".to_string(),
            db_path:     "./icm.db".to_string(),
            drbd_dir:    "/etc/drbd.d".to_string(),
            quadlet_dir: "/etc/containers/systemd".to_string(),
            temp_dir:    "/tmp/icm".to_string(),
            deploy_mode: "test".to_string(),
            pcsd_url:    "https://localhost:2224".to_string(),
            pcsd_user:   "hacluster".to_string(),
            pcsd_pass:   String::new(),
        };
    }

    Config {
        host: cfg.get("server", "host")
            .unwrap_or_else(|| "0.0.0.0".to_string()),
        port: cfg.get("server", "port")
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(5000),
        log_level: cfg.get("logging", "level")
            .unwrap_or_else(|| "info".to_string()),
        db_path: cfg.get("database", "path")
            .unwrap_or_else(|| "./icm.db".to_string()),
        drbd_dir: cfg.get("paths", "drbd_dir")
            .unwrap_or_else(|| "/etc/drbd.d".to_string()),
        quadlet_dir: cfg.get("paths", "quadlet_dir")
            .unwrap_or_else(|| "/etc/containers/systemd".to_string()),
        temp_dir: cfg.get("paths", "temp_dir")
            .unwrap_or_else(|| "/tmp/icm".to_string()),
        deploy_mode: cfg.get("paths", "deploy_mode")
            .unwrap_or_else(|| "test".to_string()),
        pcsd_url: cfg.get("pacemaker", "pcsd_url")
            .unwrap_or_else(|| "https://localhost:2224".to_string()),
        pcsd_user: cfg.get("pacemaker", "pcsd_user")
            .unwrap_or_else(|| "hacluster".to_string()),
        pcsd_pass: cfg.get("pacemaker", "pcsd_password")
            .unwrap_or_default(),
    }
}

#[tokio::main]
async fn main() {
    let config = load_config();

    // 로깅 초기화
    let effective_log_level = std::env::var("RUST_LOG").unwrap_or(config.log_level);
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&effective_log_level))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("{} 설정 로드 완료", CONFIG_FILE);

    // temp_dir 생성
    if config.deploy_mode == "test" {
        if let Err(e) = std::fs::create_dir_all(&config.temp_dir) {
            tracing::warn!("temp_dir 생성 실패 {}: {}", config.temp_dir, e);
        } else {
            tracing::info!("temp_dir: {}", config.temp_dir);
        }
    }

    // SQLite DB 초기화
    let conn = match db::init_db(&config.db_path) {
        Ok(c) => {
            tracing::info!("DB 초기화 완료: {}", config.db_path);
            c
        }
        Err(e) => {
            tracing::error!("DB 초기화 실패: {}", e);
            std::process::exit(1);
        }
    };

    let db = Arc::new(Mutex::new(conn));

    // Tera 템플릿 엔진 초기화
    let mut tera = match Tera::new("templates/**/*.html") {
        Ok(t) => {
            tracing::info!("Tera 템플릿 로드 완료");
            t
        }
        Err(e) => {
            tracing::error!("Tera 템플릿 로드 실패: {}", e);
            std::process::exit(1);
        }
    };
    tera.register_filter("starts_with", filter_starts_with);
    tera.register_filter("ends_with", filter_ends_with);

    let state = AppState {
        tera:        Arc::new(tera),
        db:          db.clone(),
        temp_dir:    config.temp_dir.clone(),
        deploy_mode: config.deploy_mode.clone(),
    };

    // 백그라운드에서 시스템 스캔 수행
    let scan_cfg = scan::ScanConfig {
        drbd_dir:    config.drbd_dir.clone(),
        quadlet_dir: config.quadlet_dir.clone(),
        pcsd_url:    config.pcsd_url.clone(),
        pcsd_user:   config.pcsd_user.clone(),
        pcsd_pass:   config.pcsd_pass.clone(),
        temp_dir:    config.temp_dir.clone(),
    };
    let db_for_scan = db.clone();
    tokio::spawn(async move {
        scan::startup_scan(scan_cfg, db_for_scan).await;
    });

    // 라우터 설정
    let app = Router::new()
        // 메인
        .route("/", get(routes::main::index))
        // DRBD
        .route("/drbd/", get(routes::drbd::index))
        .route("/drbd/scan", get(routes::drbd::scan))
        .route("/api/lvscan", get(routes::drbd::api_lvscan))
        .route("/drbd/generate", post(routes::drbd::generate))
        .route("/drbd/download", post(routes::drbd::download_res))
        .route("/drbd/save", post(routes::drbd::save))
        .route("/drbd/delete/:name", delete(routes::drbd::delete_saved))
        // Quadlet (Pod 설정)
        .route("/quadlet/", get(routes::quadlet::index))
        .route("/quadlet/generate", post(routes::quadlet::generate))
        // Container (컨테이너 설정)
        .route("/container/", get(routes::container::index))
        .route("/container/generate", post(routes::container::generate))
        .route("/container/from-inspect", post(routes::container::from_inspect))
        // Pacemaker
        .route("/pacemaker/", get(routes::pacemaker::index))
        .route("/pacemaker/generate", post(routes::pacemaker::generate))
        .route("/api/pacemaker/pcsd-fetch", post(routes::pacemaker::api_pcsd_fetch))
        // 클러스터 현황 (토폴로지 뷰어)
        .route("/cluster/", get(routes::cluster::index))
        // nftables 방화벽
        .route("/nft/", get(routes::nft::index))
        .route("/nft/generate", post(routes::nft::generate))
        // nft API
        .route("/api/nft/subnet-groups", get(routes::nft::api_list_subnet_groups))
        .route("/api/nft/subnet-groups", post(routes::nft::api_upsert_subnet_group))
        .route("/api/nft/subnet-groups/:id", delete(routes::nft::api_delete_subnet_group))
        .route("/api/nft/services", get(routes::nft::api_list_services))
        .route("/api/nft/services", post(routes::nft::api_upsert_service))
        .route("/api/nft/services/:id", delete(routes::nft::api_delete_service))
        // nft targets, global config, scan
        .route("/api/nft/targets", get(routes::nft::api_list_targets))
        .route("/api/nft/targets", post(routes::nft::api_upsert_target))
        .route("/api/nft/targets/:id", delete(routes::nft::api_delete_target))
        .route("/api/nft/config", get(routes::nft::api_get_global_config))
        .route("/api/nft/config", post(routes::nft::api_update_global_config))
        .route("/api/nft/scan", post(routes::nft::api_scan))
        // Volume
        .route("/volume/", get(routes::volume::index))
        .route("/volume/generate", post(routes::volume::generate))
        .route("/api/volumes", get(routes::volume::api_list))
        .route("/api/volumes", post(routes::volume::api_upsert))
        .route("/api/volumes/:id", delete(routes::volume::api_delete))
        .route("/api/drbd/resources", get(routes::volume::api_list_drbd_resources))
        // 노드 풀 페이지
        .route("/nodes/", get(routes::nodes::index))
        .route("/nodes/scan", post(routes::nodes::rescan))
        // 노드 API
        .route("/api/nodes", get(routes::nodes::api_list_nodes))
        .route("/api/nodes", post(routes::nodes::api_upsert_node))
        .route("/api/nodes/:id", delete(routes::nodes::api_delete_node))
        // 네트워크 API
        .route("/api/networks", get(routes::nodes::api_list_networks))
        .route("/api/networks", post(routes::nodes::api_upsert_network))
        .route("/api/networks/:id", delete(routes::nodes::api_delete_network))
        // Ansible 프로파일 API
        .route("/api/ansible-profiles", get(routes::nodes::api_list_profiles))
        .route("/api/ansible-profiles", post(routes::nodes::api_upsert_profile))
        .route("/api/ansible-profiles/:id", delete(routes::nodes::api_delete_profile))
        // 노드 인터페이스 API
        .route("/api/node-interfaces", get(routes::nodes::api_list_node_interfaces))
        .route("/api/collect-interfaces", post(routes::nodes::api_collect_interfaces))
        // Quadlet Pod 스캔 API
        .route("/api/quadlet-pods", get(routes::nodes::api_list_quadlet_pods))
        // 정적 파일
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state);

    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("서버 시작: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        tracing::error!("바인딩 실패 {}: {}", addr, e);
        std::process::exit(1);
    });
    axum::serve(listener, app).await.unwrap();
}

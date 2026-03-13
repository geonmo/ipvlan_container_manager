mod models;
mod generators;
mod routes;

use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tera::Tera;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub tera: Arc<Tera>,
}

#[tokio::main]
async fn main() {
    // 로깅 초기화
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Tera 템플릿 엔진 초기화
    let tera = match Tera::new("templates/**/*.html") {
        Ok(t) => {
            tracing::info!("Tera 템플릿 로드 완료");
            t
        }
        Err(e) => {
            tracing::error!("Tera 템플릿 로드 실패: {}", e);
            std::process::exit(1);
        }
    };

    let state = AppState {
        tera: Arc::new(tera),
    };

    // 라우터 설정
    let app = Router::new()
        // 메인
        .route("/", get(routes::main::index))
        // DRBD
        .route("/drbd/", get(routes::drbd::index))
        .route("/drbd/generate", post(routes::drbd::generate))
        .route("/drbd/download", post(routes::drbd::download_res))
        // Quadlet
        .route("/quadlet/", get(routes::quadlet::index))
        .route("/quadlet/generate", post(routes::quadlet::generate))
        .route("/quadlet/from-inspect", post(routes::quadlet::from_inspect))
        // Pacemaker
        .route("/pacemaker/", get(routes::pacemaker::index))
        .route("/pacemaker/generate", post(routes::pacemaker::generate))
        // 정적 파일
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state);

    let addr = "0.0.0.0:5000";
    tracing::info!("서버 시작: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::quadlet::{QuadletConfig, QuadletContainer};
use crate::generators::quadlet::{generate_all_units, podman_inspect_to_quadlet};
use crate::routes::quadlet::{QuadletFullForm, generate_quadlet_ansible_playbook};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("container/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─── 컨테이너 설정 폼 ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ContainerFullForm {
    // 컨테이너 JSON 배열
    pub containers_json: Option<String>,
    // Pod 이름 (참조용 — 컨테이너에 Pod= 지정 시 사용)
    pub pod_name: Option<String>,
    // Ansible 설정
    pub ansible_hosts: Option<String>,
    pub ansible_user: Option<String>,
    pub ansible_ssh_key: Option<String>,
}

impl ContainerFullForm {
    /// generate_quadlet_ansible_playbook 에서 사용하는 QuadletFullForm 으로 변환
    fn as_quadlet_form(&self) -> QuadletFullForm {
        QuadletFullForm {
            net_driver: None,
            net_interface: None,
            net_subnet: None,
            net_gateway: None,
            net_subnet6: None,
            net_gateway6: None,
            net_ipvlan_mode: None,
            pod_name: self.pod_name.clone(),
            pod_description: None,
            pod_hostname: None,
            pod_shm_size: None,
            pod_networks_json: None,
            ansible_hosts: self.ansible_hosts.clone(),
            ansible_user: self.ansible_user.clone(),
            ansible_ssh_key: self.ansible_ssh_key.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ContainerResult {
    pub files: Vec<(String, String)>,
    pub ansible_playbook: String,
}

/// POST /container/generate
pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<ContainerFullForm>,
) -> Html<String> {
    let mut config = QuadletConfig::new();

    // 컨테이너 JSON 파싱 — 폼 기반 UI(serializeContainerForm())가 보내는
    // QuadletContainer JSON 배열. podman inspect 원본 JSON은 별도
    // 엔드포인트(POST /container/from-inspect, `from_inspect()`)가 다룬다 —
    // 여기서 podman inspect 파싱을 먼저 시도하면 안 된다: 어떤 JSON 배열이든
    // (필드가 없으면 "unknown"/빈 문자열로 기본값 채워) 항상 성공해버려서,
    // 정상적인 QuadletContainer 배열 입력이 매번 빈 "unknown.container"로
    // 잘못 해석되는 실제 버그가 있었다.
    let containers: Vec<QuadletContainer> = if let Some(json_str) = &form.containers_json {
        if !json_str.trim().is_empty() {
            match serde_json::from_str::<Vec<QuadletContainer>>(json_str) {
                Ok(c) => c,
                Err(e) => {
                    let mut ctx = Context::new();
                    ctx.insert("error", &format!("JSON 파싱 오류: {}", e));
                    let rendered = state.tera.render("container/result.html", &ctx)
                        .unwrap_or_else(|e2| format!("<pre>Template error: {}</pre>", e2));
                    return Html(rendered);
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // pod_name이 있으면 컨테이너에 Pod 필드 설정
    let pod_name = form.pod_name.clone().filter(|s| !s.trim().is_empty());
    for mut c in containers {
        if c.pod.is_none() {
            c.pod = pod_name.clone();
        }
        config.containers.push(c);
    }

    let files = generate_all_units(&config);
    let quadlet_form = form.as_quadlet_form();
    let ansible_playbook = generate_quadlet_ansible_playbook(&config, &files, &quadlet_form);

    let result = ContainerResult {
        files,
        ansible_playbook,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);

    let rendered = state.tera.render("container/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

/// POST /container/from-inspect  (기존 /quadlet/from-inspect 이동)
pub async fn from_inspect(
    State(state): State<AppState>,
    Form(form): Form<InspectForm>,
) -> Html<String> {
    let mut ctx = Context::new();
    match podman_inspect_to_quadlet(&form.inspect_json) {
        Ok(containers) => {
            let mut config = QuadletConfig::new();
            config.containers = containers;
            let files = generate_all_units(&config);
            ctx.insert("files", &files);
            ctx.insert("error", &serde_json::Value::Null);
        }
        Err(e) => {
            ctx.insert("files", &Vec::<(String, String)>::new());
            ctx.insert("error", &e.to_string());
        }
    }
    let rendered = state.tera.render("container/inspect_result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

#[derive(Debug, Deserialize)]
pub struct InspectForm {
    pub inspect_json: String,
}

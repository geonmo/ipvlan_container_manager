use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::quadlet::{
    QuadletConfig, QuadletNetwork, QuadletPod, QuadletVolume,
};
use crate::generators::quadlet::{generate_all_units, podman_inspect_to_quadlet};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("quadlet/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─── Volume 폼 ────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct VolumeForm {
    pub name: String,
    pub driver: Option<String>,
    pub options: Option<String>,  // 줄바꿈 구분
}

// ─── Network 폼 ───────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct NetworkForm {
    pub name: String,
    pub driver: String,       // ipvlan or macvlan
    pub interface: String,
    pub subnet: String,
    pub gateway: Option<String>,
    pub ipvlan_mode: Option<String>,
}

// ─── Pod 폼 ───────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct PodForm {
    pub name: String,
    pub network: Option<String>,
    pub publish_ports: Option<String>,
}

// ─── Container 폼 ─────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ContainerForm {
    pub name: String,
    pub image: String,
    pub pod: Option<String>,
    pub network: Option<String>,
    pub ip: Option<String>,
    pub environment: Option<String>, // KEY=VALUE 줄바꿈 구분
    pub volumes: Option<String>,     // 줄바꿈 구분
    pub publish_ports: Option<String>,
    pub exec: Option<String>,
    pub user: Option<String>,
    pub depends_on: Option<String>,
    pub drbd_resource: Option<String>,
    pub extra_args: Option<String>,
}

// ─── 전체 설정 폼 ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct QuadletFullForm {
    // Volume
    pub vol_name: Option<String>,
    pub vol_driver: Option<String>,
    pub vol_options: Option<String>,

    // Network
    pub net_name: Option<String>,
    pub net_driver: Option<String>,
    pub net_interface: Option<String>,
    pub net_subnet: Option<String>,
    pub net_gateway: Option<String>,
    pub net_ipvlan_mode: Option<String>,

    // Pod
    pub pod_name: Option<String>,
    pub pod_network: Option<String>,
    pub pod_ports: Option<String>,

    // Container
    pub containers_json: Option<String>, // JSON 배열

    // Ansible 인벤토리
    pub ansible_hosts: Option<String>, // IP 줄바꿈
    pub ansible_user: Option<String>,
    pub ansible_ssh_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QuadletResult {
    pub files: Vec<(String, String)>,
    pub ansible_playbook: String,
}

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<QuadletFullForm>,
) -> Html<String> {
    let mut config = QuadletConfig::new();

    // Volume 처리
    if let Some(vname) = &form.vol_name {
        if !vname.trim().is_empty() {
            config.volumes.push(QuadletVolume {
                name: vname.trim().to_string(),
                driver: form.vol_driver.clone().unwrap_or_else(|| "local".to_string()),
                options: parse_lines(form.vol_options.as_deref()),
                labels: Vec::new(),
            });
        }
    }

    // Network 처리
    if let Some(nname) = &form.net_name {
        if !nname.trim().is_empty() {
            config.networks.push(QuadletNetwork {
                name: nname.trim().to_string(),
                driver: form.net_driver.clone().unwrap_or_else(|| "ipvlan".to_string()),
                interface: form.net_interface.clone().unwrap_or_default(),
                subnet: form.net_subnet.clone().unwrap_or_default(),
                gateway: form.net_gateway.clone().unwrap_or_default(),
                ipvlan_mode: form.net_ipvlan_mode.clone().unwrap_or_else(|| "l2".to_string()),
                options: Vec::new(),
            });
        }
    }

    // Pod 처리
    if let Some(pname) = &form.pod_name {
        if !pname.trim().is_empty() {
            config.pods.push(QuadletPod {
                name: pname.trim().to_string(),
                network: form.pod_network.clone().filter(|s| !s.trim().is_empty()),
                publish_ports: parse_lines(form.pod_ports.as_deref()),
                labels: Vec::new(),
            });
        }
    }

    // Container JSON 처리
    if let Some(json_str) = &form.containers_json {
        if !json_str.trim().is_empty() {
            match podman_inspect_to_quadlet(json_str) {
                Ok(containers) => config.containers.extend(containers),
                Err(e) => {
                    let mut ctx = Context::new();
                    ctx.insert("error", &format!("JSON 파싱 오류: {}", e));
                    let rendered = state.tera.render("quadlet/result.html", &ctx)
                        .unwrap_or_else(|e2| format!("<pre>Template error: {}</pre>", e2));
                    return Html(rendered);
                }
            }
        }
    }

    let files = generate_all_units(&config);
    let ansible_playbook = generate_quadlet_ansible_playbook(&config, &files, &form);

    let result = QuadletResult {
        files,
        ansible_playbook,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);

    let rendered = state.tera.render("quadlet/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

/// podman inspect JSON으로부터 Quadlet 유닛 생성
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
    let rendered = state.tera.render("quadlet/inspect_result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

#[derive(Debug, Deserialize)]
pub struct InspectForm {
    pub inspect_json: String,
}

// ─── 헬퍼 ─────────────────────────────────────────────────────

fn parse_lines(s: Option<&str>) -> Vec<String> {
    s.unwrap_or("")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn generate_quadlet_ansible_playbook(
    config: &QuadletConfig,
    files: &[(String, String)],
    form: &QuadletFullForm,
) -> String {
    let hosts = form
        .ansible_hosts
        .as_deref()
        .unwrap_or("all")
        .trim()
        .to_string();
    let user = form
        .ansible_user
        .as_deref()
        .unwrap_or("root")
        .to_string();
    let key = form
        .ansible_ssh_key
        .as_deref()
        .unwrap_or("~/.ssh/id_rsa")
        .to_string();

    let mut tasks: Vec<String> = Vec::new();
    tasks.push(format!(
        "    - name: Quadlet 디렉토리 생성\n      file:\n        path: {}\n        state: directory\n        mode: '0755'",
        config.install_path
    ));

    for (filename, content) in files {
        let escaped = content.replace('\'', "'\"'\"'");
        tasks.push(format!(
            "    - name: {} 배포\n      copy:\n        dest: {}/{}\n        content: |\n{}",
            filename,
            config.install_path,
            filename,
            content
                .lines()
                .map(|l| format!("          {}", l))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        let _ = escaped;
    }

    tasks.push(
        "    - name: systemd 데몬 리로드\n      systemd:\n        daemon_reload: yes".to_string(),
    );

    format!(
        "---\n- name: Quadlet 유닛 배포\n  hosts: {}\n  remote_user: {}\n  become: yes\n  vars:\n    ansible_ssh_private_key_file: {}\n  tasks:\n{}",
        hosts,
        user,
        key,
        tasks.join("\n\n")
    )
}

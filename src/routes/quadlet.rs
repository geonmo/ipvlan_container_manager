use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::quadlet::{
    QuadletConfig, QuadletNetwork, QuadletPod, PodNetworkEntry,
};
use crate::generators::quadlet::generate_all_units;

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
    pub ip: Option<String>,
}

// ─── 전체 설정 폼 (Pod 설정 전용) ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct QuadletFullForm {
    // Network (단일 네트워크 정의 — 풀에서 선택되지 않을 때 직접 입력용)
    pub net_name: Option<String>,
    pub net_driver: Option<String>,
    pub net_interface: Option<String>,
    pub net_subnet: Option<String>,
    pub net_gateway: Option<String>,
    pub net_subnet6: Option<String>,
    pub net_gateway6: Option<String>,
    pub net_ipvlan_mode: Option<String>,

    // Pod
    pub pod_name: Option<String>,
    // pod_networks_json: JSON 배열 [{"network":"ipvlan0","ip":"192.168.1.100"}, ...]
    pub pod_networks_json: Option<String>,

    // Ansible 인벤토리
    pub ansible_hosts: Option<String>,
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

    // Network 처리: pod_networks_json에서 네트워크 이름 목록 수집 → DB에서 조회
    let pod_networks: Vec<PodNetworkEntry> = if let Some(json_str) = &form.pod_networks_json {
        if !json_str.trim().is_empty() {
            serde_json::from_str(json_str).unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // 각 네트워크를 DB에서 조회해 config.networks에 추가
    {
        let db = state.db.lock().unwrap();
        for entry in &pod_networks {
            if entry.network.is_empty() {
                continue;
            }
            // DB에서 해당 이름의 네트워크 조회
            let result: rusqlite::Result<crate::db::DbNetwork> = db.query_row(
                "SELECT id, name, driver, interface, subnet, gateway, subnet6, gateway6, ipv6, ipvlan_mode, source, created_at FROM networks WHERE name = ?1 LIMIT 1",
                rusqlite::params![entry.network],
                |row| {
                    Ok(crate::db::DbNetwork {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        driver: row.get(2)?,
                        interface: row.get(3)?,
                        subnet: row.get(4)?,
                        gateway: row.get(5)?,
                        subnet6: row.get(6)?,
                        gateway6: row.get(7)?,
                        ipv6: row.get(8)?,
                        ipvlan_mode: row.get(9)?,
                        source: row.get(10)?,
                        created_at: row.get(11)?,
                    })
                },
            );
            if let Ok(db_net) = result {
                // 이미 추가된 네트워크 중복 방지
                if !config.networks.iter().any(|n| n.name == db_net.name) {
                    config.networks.push(QuadletNetwork {
                        name: db_net.name,
                        driver: db_net.driver,
                        interface: db_net.interface,
                        subnet: db_net.subnet,
                        gateway: db_net.gateway,
                        subnet6: db_net.subnet6,
                        gateway6: db_net.gateway6,
                        ipv6: db_net.ipv6,
                        ipvlan_mode: db_net.ipvlan_mode,
                        options: Vec::new(),
                    });
                }
            } else {
                // DB에 없으면 net_name 등 직접 입력 필드에서 추가 (풀 미등록 네트워크)
                if !config.networks.iter().any(|n| n.name == entry.network) {
                    let subnet6  = form.net_subnet6.clone().unwrap_or_default();
                    let gateway6 = form.net_gateway6.clone().unwrap_or_default();
                    let ipv6     = !subnet6.is_empty();
                    config.networks.push(QuadletNetwork {
                        name: entry.network.clone(),
                        driver: form.net_driver.clone().unwrap_or_else(|| "ipvlan".to_string()),
                        interface: form.net_interface.clone().unwrap_or_default(),
                        subnet: form.net_subnet.clone().unwrap_or_default(),
                        gateway: form.net_gateway.clone().unwrap_or_default(),
                        subnet6,
                        gateway6,
                        ipv6,
                        ipvlan_mode: form.net_ipvlan_mode.clone().unwrap_or_else(|| "l2".to_string()),
                        options: Vec::new(),
                    });
                }
            }
        }
    }

    // Pod 처리
    if let Some(pname) = &form.pod_name {
        if !pname.trim().is_empty() {
            config.pods.push(QuadletPod {
                name: pname.trim().to_string(),
                networks: pod_networks,
                labels: Vec::new(),
            });
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

// ─── 헬퍼 ─────────────────────────────────────────────────────

fn sanitize_var(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

pub fn generate_quadlet_ansible_playbook(
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

    // 인터페이스 자동감지가 필요한 네트워크 파일 처리
    let mut iface_detect_tasks: Vec<String> = Vec::new();
    let mut resolved_files: Vec<(String, String)> = Vec::new();

    for (filename, content) in files {
        if content.contains("__PARENT_IFACE__") {
            let net_name = filename.trim_end_matches(".network");
            let var_name = format!("_piface_{}", sanitize_var(net_name));
            // IPv4 서브넷 우선, 없으면 IPv6
            let subnet = config.networks.iter()
                .find(|n| n.name == net_name)
                .map(|n| if !n.subnet.is_empty() { n.subnet.as_str() } else { n.subnet6.as_str() })
                .unwrap_or("0.0.0.0/0");
            iface_detect_tasks.push(format!(
                "    - name: {} 부모 인터페이스 감지\n      shell: ip route show to match {} | grep -oP 'dev \\K\\S+' | head -1\n      register: {}\n      changed_when: false\n      failed_when: false",
                net_name, subnet, var_name
            ));
            let new_content = content.replace(
                "Options=parent=__PARENT_IFACE__",
                &format!("Options=parent={{{{ {}.stdout | trim }}}}", var_name),
            );
            resolved_files.push((filename.clone(), new_content));
        } else {
            resolved_files.push((filename.clone(), content.clone()));
        }
    }

    let mut tasks: Vec<String> = Vec::new();
    tasks.push(format!(
        "    - name: Quadlet 디렉토리 생성\n      file:\n        path: {}\n        state: directory\n        mode: '0755'",
        config.install_path
    ));

    // 인터페이스 감지 태스크를 복사 태스크보다 먼저 추가
    tasks.extend(iface_detect_tasks);

    for (filename, content) in &resolved_files {
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

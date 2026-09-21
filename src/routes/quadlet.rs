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
    pub net_driver: Option<String>,
    pub net_interface: Option<String>,
    pub net_subnet: Option<String>,
    pub net_gateway: Option<String>,
    pub net_subnet6: Option<String>,
    pub net_gateway6: Option<String>,
    pub net_ipvlan_mode: Option<String>,

    // Pod
    pub pod_name: Option<String>,
    pub pod_description: Option<String>,
    pub pod_hostname: Option<String>,
    pub pod_shm_size: Option<String>,
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
    let mut pod_networks: Vec<PodNetworkEntry> = if let Some(json_str) = &form.pod_networks_json {
        if !json_str.trim().is_empty() {
            serde_json::from_str(json_str).unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // 각 네트워크를 DB에서 조회해 config.networks에 추가
    // (gateway 정보도 수집해서 나중에 pod_networks에 주입)
    let mut gw_map: Vec<(String, String, String)> = Vec::new(); // (name, gateway, gateway6)
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
                        name: db_net.name.clone(),
                        driver: db_net.driver,
                        interface: db_net.interface,
                        subnet: db_net.subnet,
                        gateway: db_net.gateway.clone(),
                        subnet6: db_net.subnet6,
                        gateway6: db_net.gateway6.clone(),
                        ipv6: db_net.ipv6,
                        ipvlan_mode: db_net.ipvlan_mode,
                        internal: false,
                        options: Vec::new(),
                    });
                }
                // gateway 정보를 나중에 주입하기 위해 저장
                gw_map.push((db_net.name.clone(), db_net.gateway.clone(), db_net.gateway6.clone()));
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
                        internal: false,
                        options: Vec::new(),
                    });
                }
            }
        }
    }

    // PodNetworkEntry에 gateway 정보 자동 주입
    for (net_name, gw, gw6) in &gw_map {
        if let Some(pne) = pod_networks.iter_mut().find(|e| &e.network == net_name) {
            if pne.gateway.is_none() && !gw.is_empty() {
                pne.gateway = Some(gw.clone());
            }
            if pne.gateway6.is_none() && !gw6.is_empty() {
                pne.gateway6 = Some(gw6.clone());
            }
        }
    }

    // Pod 처리
    if let Some(pname) = &form.pod_name {
        if !pname.trim().is_empty() {
            config.pods.push(QuadletPod {
                name: pname.trim().to_string(),
                description: form.pod_description.clone().filter(|s| !s.trim().is_empty()),
                hostname: form.pod_hostname.clone().filter(|s| !s.trim().is_empty()),
                shm_size: form.pod_shm_size.clone().filter(|s| !s.trim().is_empty()),
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

/// 폼에서 받은 Ansible 호스트 패턴을 정규화한다.
///
/// 두 가지를 방어한다:
/// 1. HTML 폼은 빈 입력을 `Some("")`로 보내므로 `unwrap_or("all")`만으로는
///    기본값이 적용되지 않는다 → `hosts: ` (빈 값) 플레이북이 만들어진다.
/// 2. 노드 선택 UI가 여러 호스트를 줄바꿈으로 넘기면 `hosts: node1\nnode2`가
///    되어 YAML 자체가 깨진다. Ansible 호스트 패턴은 쉼표 구분이다.
fn normalize_ansible_hosts(raw: Option<&str>) -> String {
    let parts: Vec<&str> = raw
        .unwrap_or("")
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        "all".to_string()
    } else {
        parts.join(",")
    }
}

/// 빈 문자열(HTML 폼의 미입력)일 때 기본값으로 떨어지도록 하는 헬퍼.
fn or_default(raw: Option<&str>, default: &str) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
}

pub fn generate_quadlet_ansible_playbook(
    config: &QuadletConfig,
    files: &[(String, String)],
    form: &QuadletFullForm,
) -> String {
    let hosts = normalize_ansible_hosts(form.ansible_hosts.as_deref());
    let user = or_default(form.ansible_user.as_deref(), "root");
    let key = or_default(form.ansible_ssh_key.as_deref(), "~/.ssh/id_rsa");

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
            // __PARENT_IFACE__ 만 교체 — "Options=parent=__PARENT_IFACE__,mode=l2" 등도 처리
            let new_content = content.replace(
                "__PARENT_IFACE__",
                &format!("{{{{ {}.stdout | trim }}}}", var_name),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::quadlet::QuadletContainer;

    fn empty_form() -> QuadletFullForm {
        QuadletFullForm {
            net_driver: None,
            net_interface: None,
            net_subnet: None,
            net_gateway: None,
            net_subnet6: None,
            net_gateway6: None,
            net_ipvlan_mode: None,
            pod_name: None,
            pod_description: None,
            pod_hostname: None,
            pod_shm_size: None,
            pod_networks_json: None,
            ansible_hosts: None,
            ansible_user: None,
            ansible_ssh_key: None,
        }
    }

    fn sample_config() -> QuadletConfig {
        let mut config = QuadletConfig::new();
        config.containers.push(QuadletContainer {
            name: "myapp".to_string(),
            image: "quay.io/myapp:latest".to_string(),
            ..Default::default()
        });
        config
    }

    fn playbook_with_hosts(raw: Option<&str>) -> String {
        let mut form = empty_form();
        form.ansible_hosts = raw.map(str::to_string);
        let config = sample_config();
        let files = crate::generators::quadlet::generate_all_units(&config);
        generate_quadlet_ansible_playbook(&config, &files, &form)
    }

    #[test]
    fn normalize_hosts_falls_back_to_all_when_missing_or_blank() {
        // HTML 폼은 미입력 필드를 Some("")로 보내므로 None뿐 아니라
        // 빈 문자열/공백도 기본값으로 떨어져야 한다.
        assert_eq!(normalize_ansible_hosts(None), "all");
        assert_eq!(normalize_ansible_hosts(Some("")), "all");
        assert_eq!(normalize_ansible_hosts(Some("   ")), "all");
        assert_eq!(normalize_ansible_hosts(Some("\n")), "all");
    }

    #[test]
    fn normalize_hosts_converts_newline_separated_list_to_comma_pattern() {
        // 노드 선택 UI가 줄바꿈으로 넘기던 값도 안전하게 받아낸다.
        assert_eq!(normalize_ansible_hosts(Some("node1\nnode2\nnode3")), "node1,node2,node3");
        assert_eq!(normalize_ansible_hosts(Some("node1,node2")), "node1,node2");
        assert_eq!(normalize_ansible_hosts(Some(" node1 , node2 ")), "node1,node2");
        assert_eq!(normalize_ansible_hosts(Some("container_service")), "container_service");
    }

    /// 예전에는 `hosts: node1\nnode2` 가 그대로 나가 생성된 플레이북 YAML이
    /// 깨졌다 (`could not find expected ':'`).
    #[test]
    fn playbook_hosts_line_is_always_single_line() {
        let playbook = playbook_with_hosts(Some("node1\nnode2\nnode3"));
        assert!(playbook.contains("  hosts: node1,node2,node3\n"));
        assert_eq!(
            playbook.lines().filter(|l| l.starts_with("  hosts:")).count(),
            1
        );
        // 호스트 이름이 들여쓰기 없는 맨 앞 줄로 새어 나오면 안 된다.
        assert!(!playbook.lines().any(|l| l == "node2"));
    }

    #[test]
    fn playbook_falls_back_to_all_when_no_node_selected() {
        let playbook = playbook_with_hosts(Some(""));
        assert!(playbook.contains("  hosts: all\n"));
        assert!(!playbook.contains("  hosts: \n"));
    }

    #[test]
    fn playbook_user_and_key_fall_back_when_form_sends_empty_strings() {
        let mut form = empty_form();
        form.ansible_hosts = Some(String::new());
        form.ansible_user = Some(String::new());
        form.ansible_ssh_key = Some("   ".to_string());
        let config = sample_config();
        let files = crate::generators::quadlet::generate_all_units(&config);
        let playbook = generate_quadlet_ansible_playbook(&config, &files, &form);

        assert!(playbook.contains("  remote_user: root\n"));
        assert!(playbook.contains("    ansible_ssh_private_key_file: ~/.ssh/id_rsa\n"));
    }

    /// 생성물이 실제로 Ansible에 먹히려면 우선 YAML로 파싱돼야 한다.
    #[test]
    fn generated_playbook_is_valid_yaml() {
        for hosts in [None, Some(""), Some("node1\nnode2"), Some("node1,node2")] {
            let playbook = playbook_with_hosts(hosts);
            let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&playbook);
            assert!(
                parsed.is_ok(),
                "hosts={:?} 일 때 YAML 파싱 실패: {:?}\n---\n{}",
                hosts,
                parsed.err(),
                playbook
            );
        }
    }
}

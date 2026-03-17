use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::pacemaker::{
    ClusterConfig, ClusterNode, ColocationConstraint, DrbdPacemakerResource,
    LocationConstraint, OrderConstraint, PacemakerConfig, SystemdResource, SystemdResourceType,
};
use crate::generators::pacemaker::{
    generate_pcs_script, generate_default_constraints, generate_cib_xml_snippet,
};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("pacemaker/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

// ─── 폼 데이터 ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PacemakerFormData {
    // 클러스터 기본 설정
    pub cluster_name: String,
    /// JSON 배열: [{hostname, ip}]
    pub nodes_json: Option<String>,
    pub stonith_enabled: Option<String>,
    pub no_quorum_policy: String,
    pub migration_threshold: Option<u32>,
    pub failure_timeout: Option<String>,

    // DRBD 리소스
    pub drbd_resource_name: String,        // pacemaker 리소스 이름
    pub drbd_res_name: String,             // DRBD .res의 resource 이름
    pub drbd_clone_name: String,
    pub drbd_notify: Option<String>,
    pub drbd_on_fail: Option<String>,
    pub drbd_preferred_primary: Option<String>,

    // Systemd 리소스 목록 (JSON 배열)
    pub systemd_resources_json: Option<String>,

    // 제약조건 자동 생성 여부
    pub auto_constraints: Option<String>,

    // 수동 Order 제약조건
    pub order_constraints_json: Option<String>,

    // 수동 Colocation 제약조건
    pub colocation_constraints_json: Option<String>,

    // 수동 Location 제약조건
    pub location_constraints_json: Option<String>,

    // Ansible
    pub ansible_hosts: Option<String>,
    pub ansible_user: Option<String>,
    pub ansible_ssh_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SystemdResourceInput {
    pub resource_name: String,
    pub systemd_unit: String,
    pub resource_type: String, // "container", "pod", "volume"
    pub monitor_interval: Option<String>,
    pub start_timeout: Option<String>,
    pub stop_timeout: Option<String>,
    pub clone: Option<bool>,
    pub clone_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PacemakerResult {
    pub pcs_script: String,
    pub cib_xml: String,
    pub ansible_playbook: String,
}

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<PacemakerFormData>,
) -> Html<String> {
    let mut config = PacemakerConfig::default();

    // nodes_json 파싱: [{hostname, ip}]
    let parsed_nodes: Vec<ClusterNode> = form.nodes_json.as_deref()
        .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let name = v.get("hostname").and_then(|h| h.as_str())?.to_string();
            if name.is_empty() { return None; }
            Some(ClusterNode { name, id: (i + 1) as u32 })
        })
        .collect();

    // 클러스터 기본 설정
    config.cluster = ClusterConfig {
        cluster_name: form.cluster_name.clone(),
        nodes: parsed_nodes,
        stonith_enabled: form.stonith_enabled.as_deref() == Some("on"),
        no_quorum_policy: form.no_quorum_policy.clone(),
        migration_threshold: form.migration_threshold.unwrap_or(3),
        failure_timeout: form.failure_timeout.clone().unwrap_or_else(|| "60s".to_string()),
    };

    // DRBD 리소스
    config.drbd_resources.push(DrbdPacemakerResource {
        resource_name: form.drbd_resource_name.clone(),
        drbd_resource_name: form.drbd_res_name.clone(),
        clone_name: form.drbd_clone_name.clone(),
        notify: form.drbd_notify.as_deref() == Some("on"),
        promotable: true,
        on_fail: form.drbd_on_fail.clone().unwrap_or_else(|| "fence".to_string()),
        target_role_master_node: form
            .drbd_preferred_primary
            .clone()
            .filter(|s| !s.trim().is_empty()),
    });

    // Systemd 리소스 파싱
    if let Some(json) = &form.systemd_resources_json {
        if !json.trim().is_empty() {
            if let Ok(inputs) = serde_json::from_str::<Vec<SystemdResourceInput>>(json) {
                for input in inputs {
                    let rtype = match input.resource_type.to_lowercase().as_str() {
                        "pod" => SystemdResourceType::Pod,
                        "volume" => SystemdResourceType::Volume,
                        _ => SystemdResourceType::Container,
                    };
                    config.systemd_resources.push(SystemdResource {
                        resource_name: input.resource_name,
                        systemd_unit: input.systemd_unit,
                        resource_type: rtype,
                        monitor_interval: input
                            .monitor_interval
                            .unwrap_or_else(|| "30s".to_string()),
                        start_timeout: input
                            .start_timeout
                            .unwrap_or_else(|| "60s".to_string()),
                        stop_timeout: input
                            .stop_timeout
                            .unwrap_or_else(|| "60s".to_string()),
                        clone: input.clone.unwrap_or(false),
                        clone_name: input.clone_name,
                    });
                }
            }
        }
    }

    // 자동 제약조건 생성
    if form.auto_constraints.as_deref() == Some("on") {
        generate_default_constraints(&mut config);
    }

    // 수동 Order 제약조건
    if let Some(json) = &form.order_constraints_json {
        if !json.trim().is_empty() {
            if let Ok(orders) = serde_json::from_str::<Vec<OrderConstraint>>(json) {
                config.order_constraints.extend(orders);
            }
        }
    }

    // 수동 Colocation 제약조건
    if let Some(json) = &form.colocation_constraints_json {
        if !json.trim().is_empty() {
            if let Ok(cols) = serde_json::from_str::<Vec<ColocationConstraint>>(json) {
                config.colocation_constraints.extend(cols);
            }
        }
    }

    // 수동 Location 제약조건
    if let Some(json) = &form.location_constraints_json {
        if !json.trim().is_empty() {
            if let Ok(locs) = serde_json::from_str::<Vec<LocationConstraint>>(json) {
                config.location_constraints.extend(locs);
            }
        }
    }

    let pcs_script = generate_pcs_script(&config);
    let cib_xml = generate_cib_xml_snippet(&config);
    let ansible_playbook = generate_pacemaker_ansible_playbook(&form, &pcs_script);

    let result = PacemakerResult {
        pcs_script,
        cib_xml,
        ansible_playbook,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);

    let rendered = state.tera.render("pacemaker/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

fn generate_pacemaker_ansible_playbook(
    form: &PacemakerFormData,
    pcs_script: &str,
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

    let script_lines: String = pcs_script
        .lines()
        .map(|l| format!("          {}", l))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"---
- name: Pacemaker 설정 적용
  hosts: {hosts}
  remote_user: {user}
  become: yes
  vars:
    ansible_ssh_private_key_file: {key}
  tasks:
    - name: pacemaker 및 corosync 패키지 설치
      dnf:
        name:
          - pacemaker
          - pcs
          - corosync
        state: present

    - name: pcsd 서비스 활성화 및 시작
      systemd:
        name: pcsd
        state: started
        enabled: yes

    - name: pcs 설정 스크립트 복사
      copy:
        dest: /tmp/pacemaker_setup.sh
        content: |
{script}
        mode: '0750'

    - name: pcs 설정 스크립트 실행 (첫 번째 노드에서만)
      command: /tmp/pacemaker_setup.sh
      run_once: true
      delegate_to: {node1}
"#,
        hosts = hosts,
        user = user,
        key = key,
        script = script_lines,
        node1 = form.nodes_json.as_deref()
            .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
            .and_then(|v| v.into_iter().next())
            .and_then(|n| n.get("hostname").and_then(|h| h.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "localhost".to_string()),
    )
}

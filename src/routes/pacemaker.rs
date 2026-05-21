use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    Form, Json,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::pacemaker::{
    ClusterConfig, ClusterNode, ColocationConstraint, DrbdPacemakerResource,
    FsResource, LocationConstraint, OrderConstraint, PacemakerConfig,
    ResourceGroup, SystemdResource, SystemdResourceType,
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

    // DRBD 리소스 목록 (JSON 배열) — 각 항목에 FS 리소스 포함
    pub drbd_resources_json: Option<String>,

    // Systemd 리소스 목록 (JSON 배열)
    pub systemd_resources_json: Option<String>,

    // 리소스 그룹 목록 (JSON 배열)
    pub resource_groups_json: Option<String>,

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

/// UI에서 DRBD+FS 묶음으로 입력받는 구조체
#[derive(Debug, Deserialize, Serialize)]
pub struct DrbdGroupInput {
    pub resource_name: String,          // pacemaker 리소스 이름 (drbd-r0)
    pub drbd_resource_name: String,     // .res resource 이름 (r0)
    pub clone_name: String,             // clone 이름 (drbd-r0-clone)
    pub notify: Option<bool>,
    pub on_fail: Option<String>,
    pub target_role_master_node: Option<String>,
    pub fs: Option<FsResourceInput>,    // 연결된 FS 리소스 (없으면 None)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsResourceInput {
    pub resource_name: String,
    pub device: String,
    pub directory: String,
    pub fstype: Option<String>,
    pub monitor_interval: Option<String>,
    pub start_timeout: Option<String>,
    pub stop_timeout: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResourceGroupInput {
    pub group_name: String,
    pub members: Vec<String>,    // 순서 있는 리소스 이름 목록
    pub after_fs: Option<String>,
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

    // DRBD + FS 리소스 파싱 (UI에서 묶음으로 입력)
    if let Some(json) = &form.drbd_resources_json {
        if !json.trim().is_empty() {
            if let Ok(groups) = serde_json::from_str::<Vec<DrbdGroupInput>>(json) {
                for g in groups {
                    if g.resource_name.is_empty() { continue; }
                    config.drbd_resources.push(DrbdPacemakerResource {
                        resource_name: g.resource_name.clone(),
                        drbd_resource_name: g.drbd_resource_name.clone(),
                        clone_name: g.clone_name.clone(),
                        notify: g.notify.unwrap_or(true),
                        promotable: true,
                        on_fail: g.on_fail.unwrap_or_else(|| "fence".to_string()),
                        target_role_master_node: g.target_role_master_node
                            .filter(|s| !s.trim().is_empty()),
                    });
                    if let Some(fs) = g.fs {
                        if !fs.resource_name.is_empty() {
                            config.fs_resources.push(FsResource {
                                resource_name: fs.resource_name,
                                device: fs.device,
                                directory: fs.directory,
                                fstype: fs.fstype.unwrap_or_else(|| "xfs".to_string()),
                                monitor_interval: fs.monitor_interval.unwrap_or_else(|| "20s".to_string()),
                                start_timeout: fs.start_timeout.unwrap_or_else(|| "60s".to_string()),
                                stop_timeout: fs.stop_timeout.unwrap_or_else(|| "60s".to_string()),
                                drbd_clone_name: g.clone_name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

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

    // 리소스 그룹 파싱
    if let Some(json) = &form.resource_groups_json {
        if !json.trim().is_empty() {
            if let Ok(inputs) = serde_json::from_str::<Vec<ResourceGroupInput>>(json) {
                for input in inputs {
                    if !input.group_name.is_empty() && !input.members.is_empty() {
                        config.resource_groups.push(ResourceGroup {
                            group_name: input.group_name,
                            members: input.members,
                            after_fs: input.after_fs,
                        });
                    }
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

// ── pcsd REST API 동기화 ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PcsdFetchRequest {
    pub pcsd_url: String,
}

pub async fn api_pcsd_fetch(
    Json(req): Json<PcsdFetchRequest>,
) -> impl IntoResponse {
    match pcsd_fetch_cluster_info(&req.pcsd_url).await {
        Ok(data) => Json(serde_json::json!({ "ok": true, "data": data })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        ).into_response(),
    }
}

/// `/remote/status` 엔드포인트 + 로컬 known-hosts 토큰으로 클러스터 상태 조회
async fn pcsd_fetch_cluster_info(url: &str) -> anyhow::Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let base = url.trim_end_matches('/');

    // URL 에서 호스트명 추출
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(':').next()
        .unwrap_or("localhost")
        .to_string();

    // /var/lib/pcsd/known-hosts 에서 노드 토큰 읽기
    let token = pcsd_read_node_token(&host)?;

    // /remote/status — 노드/리소스/제약조건 전체 포함
    let resp = client
        .get(&format!("{}/remote/status?version=2&operations=1", base))
        .header("Cookie", format!("token={}", token))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "클러스터 상태 조회 실패: HTTP {}",
            resp.status()
        ));
    }

    let status: serde_json::Value = resp.json().await?;

    if status.get("notauthorized").and_then(|v| v.as_str()) == Some("true") {
        return Err(anyhow::anyhow!(
            "pcsd 인증 실패: known-hosts 토큰이 유효하지 않습니다"
        ));
    }

    let cluster_name = status["cluster_name"].as_str().unwrap_or("").to_string();

    // known_nodes / corosync_online 에서 노드 목록 추출
    let nodes: Vec<serde_json::Value> = status["corosync_online"]
        .as_array()
        .into_iter().flatten()
        .filter_map(|v| v.as_str())
        .map(|n| serde_json::json!({"name": n, "addr": n}))
        .collect();

    let resources = status.get("resource_list").cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let constraints = status.get("constraints").cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let groups = status.get("groups").cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));

    Ok(serde_json::json!({
        "cluster_name": cluster_name,
        "nodes": nodes,
        "resources": resources,
        "constraints": constraints,
        "groups": groups,
        "raw_status": status,
    }))
}

/// /var/lib/pcsd/known-hosts 에서 hostname 에 해당하는 토큰 반환
/// 정확히 일치하는 호스트 없으면 첫 번째 항목 사용
fn pcsd_read_node_token(host: &str) -> anyhow::Result<String> {
    let path = "/var/lib/pcsd/known-hosts";
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("{} 읽기 실패: {}.\n클러스터 노드에서 실행해야 합니다.", path, e))?;

    let json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("{} 파싱 실패: {}", path, e))?;

    let known = json.get("known_hosts")
        .ok_or_else(|| anyhow::anyhow!("known_hosts 키 없음"))?;

    let entry = known.get(host)
        .or_else(|| known.as_object().and_then(|m| m.values().next()));

    entry
        .and_then(|e| e.get("token"))
        .and_then(|t| t.as_str())
        .map(|t| t.to_string())
        .ok_or_else(|| anyhow::anyhow!("호스트 '{}' 의 토큰을 찾을 수 없습니다", host))
}


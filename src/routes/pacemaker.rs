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
    ResourceGroup, StonithDevice, SystemdResource, SystemdResourceType,
};
use crate::generators::pacemaker::{
    generate_pcs_script, generate_default_constraints, generate_cib_xml_snippet,
    generate_maintenance_script, generate_drbd_resource_cmds, generate_fs_resource_cmds,
    generate_systemd_resource_cmds, generate_resource_group_cmd, generate_order_constraint,
    generate_colocation_constraint, generate_location_constraint, sanitize_stonith_id,
};

/// YAML 작은따옴표(single-quoted) 스칼라로 안전하게 감싼다. 리소스/제약조건
/// 이름은 사용자 입력이라 `'`가 포함될 수 있고(예: pcs 출력 검색 패턴 자체가
/// `resource '<name>'` 형태), single-quoted 스칼라 안의 `'`는 `''`로 두 번
/// 반복해야만 올바르게 이스케이프된다 (YAML 스펙).
fn yaml_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 이미 큰따옴표로 감싼 YAML 스칼라 안에 사용자 입력을 끼워 넣을 때
/// 쓰는 내부 이스케이프(따옴표 문자만 치환, 바깥 따옴표는 호출부가 직접 씀).
fn yaml_dquote_inner(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// YAML 큰따옴표(double-quoted) 스칼라로 안전하게 감싼다. 사용자 입력에
/// `\`나 `"`가 섞여도 깨지지 않도록 이스케이프한다.
fn yaml_dquote(s: &str) -> String {
    format!("\"{}\"", yaml_dquote_inner(s))
}

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

    // STONITH(fencing) 장치 목록 (JSON 배열) — PLAN.md A.5
    pub stonith_devices_json: Option<String>,

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
    pub promote_timeout: Option<String>,
    pub demote_timeout: Option<String>,
    pub start_timeout: Option<String>,
    pub stop_timeout: Option<String>,
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
    /// 롤링 유지보수 스크립트 (PLAN.md A.7) — cluster.nodes가 비어 있으면 빈 문자열.
    pub maintenance_script: String,
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
                        promote_timeout: g.promote_timeout
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "90s".to_string()),
                        demote_timeout: g.demote_timeout
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "90s".to_string()),
                        start_timeout: g.start_timeout
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "240s".to_string()),
                        stop_timeout: g.stop_timeout
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "100s".to_string()),
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

    // STONITH(fencing) 장치 파싱 (PLAN.md A.5)
    if let Some(json) = &form.stonith_devices_json {
        if !json.trim().is_empty() {
            if let Ok(devices) = serde_json::from_str::<Vec<StonithDevice>>(json) {
                config.stonith_devices = devices
                    .into_iter()
                    .filter(|d| !d.node.is_empty() && !d.ipmi_ip.is_empty())
                    .collect();
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
    let ansible_playbook = generate_pacemaker_ansible_playbook(&form, &config);
    let maintenance_script = generate_maintenance_script(&config);

    let result = PacemakerResult {
        pcs_script,
        cib_xml,
        ansible_playbook,
        maintenance_script,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);

    let rendered = state.tera.render("pacemaker/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

/// Pacemaker Ansible 플레이북 생성 (PLAN.md A.3+A.4+A.5+A.9).
///
/// R02/R11/R55 실측 패턴을 그대로 이식: (1) 부트스트랩(설치~cluster setup,
/// versionlock, 방화벽 — cluster.nodes가 비어 있으면 생략), (2) STONITH
/// 등록(stonith_devices가 비어 있으면 생략), (3) 리소스/제약조건 생성
/// (pcs resource config/pcs constraint --full을 한 번만 조회해서 register한
/// 뒤, 각 항목을 `is not search(...)`로 가드 — 멱등성 보장).
///
/// A.8: 대상 hosts가 실제 인벤토리에 resolve되는지는 이 앱이 직접 검증할
/// 방법이 없어(Ansible 실행은 사용자 쪽 인벤토리에 달림), 파일 상단에
/// `ansible-inventory --graph` 사전 점검 안내만 주석으로 남긴다.
fn generate_pacemaker_ansible_playbook(
    form: &PacemakerFormData,
    config: &PacemakerConfig,
) -> String {
    let hosts = form
        .ansible_hosts
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("all")
        .trim()
        .to_string();
    let user = form
        .ansible_user
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("root")
        .to_string();
    let key = form
        .ansible_ssh_key
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("~/.ssh/id_rsa")
        .to_string();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("# 실행 전 확인 (PLAN.md A.8): `ansible-inventory -i <inventory> --graph`로\n");
    out.push_str(&format!("# 아래 hosts 값(\"{}\")이 예상한 노드 수만큼 resolve되는지 확인하세요.\n", hosts));
    out.push_str("# 그룹이 존재하지 않으면 ansible은 에러 없이 대상 0개로 조용히 끝납니다.\n");

    let mut plays: Vec<String> = Vec::new();
    if let Some(play) = build_bootstrap_play(config, &hosts, &user, &key) {
        plays.push(play);
    }
    if let Some(play) = build_stonith_play(config, &hosts, &user, &key) {
        plays.push(play);
    }
    plays.push(build_resources_play(config, &hosts, &user, &key));

    out.push_str(&plays.join("\n"));
    out
}

/// PLAN.md A.4(부트스트랩) + A.5의 versionlock 부분 + A.9(방화벽).
/// `cluster.nodes`가 비어 있으면(사용자가 클러스터 기본 섹션을 아직 안
/// 채운 경우) 기존 사용자 흐름과의 하위 호환을 위해 생략한다.
fn build_bootstrap_play(config: &PacemakerConfig, hosts: &str, user: &str, key: &str) -> Option<String> {
    if config.cluster.nodes.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("- name: Pacemaker 클러스터 부트스트랩 (설치 ~ cluster setup)\n");
    out.push_str(&format!("  hosts: {}\n", hosts));
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));
    out.push_str("  vars_prompt:\n");
    out.push_str("    - name: hacluster_password\n");
    out.push_str("      prompt: \"hacluster 계정 비밀번호 (신규 클러스터 생성 시에만 사용, Vault 사용 시 이 프롬프트 대신 vars로 전달 가능)\"\n");
    out.push_str("      private: yes\n");
    out.push_str("  tasks:\n");

    // 1. 저장소 활성화
    out.push_str("    - name: epel-release 설치\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: epel-release\n");
    out.push_str("        state: present\n\n");
    out.push_str("    - name: crb 저장소 활성화\n");
    out.push_str("      ansible.builtin.command: dnf config-manager --set-enabled crb\n");
    out.push_str("      changed_when: true\n\n");
    out.push_str("    - name: highavailability 저장소 활성화\n");
    out.push_str("      ansible.builtin.command: dnf config-manager --set-enabled highavailability\n");
    out.push_str("      changed_when: true\n\n");

    // 2. 패키지 설치
    out.push_str("    - name: pacemaker/corosync/pcs/fence-agents-all 설치\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name:\n");
    out.push_str("          - pacemaker\n");
    out.push_str("          - corosync\n");
    out.push_str("          - pcs\n");
    out.push_str("          - fence-agents-all\n");
    out.push_str("        state: present\n\n");

    // A.5: versionlock — "유지보수 시에만"이 아니라 부트스트랩 직후 즉시.
    out.push_str("    - name: dnf-plugin-versionlock 설치\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: python3-dnf-plugin-versionlock\n");
    out.push_str("        state: present\n\n");
    out.push_str("    - name: pacemaker/corosync/pcs versionlock 설정\n");
    out.push_str("      ansible.builtin.command: \"dnf versionlock add {{ item }}\"\n");
    out.push_str("      loop:\n");
    out.push_str("        - pacemaker\n");
    out.push_str("        - corosync\n");
    out.push_str("        - pcs\n");
    out.push_str("      register: lock_result\n");
    out.push_str("      changed_when: \"'adding' in lock_result.stdout\"\n\n");

    // A.9: 방화벽
    out.push_str("    - name: corosync 포트 허용 (5404-5405/udp)\n");
    out.push_str("      ansible.posix.firewalld:\n");
    out.push_str("        zone: work\n");
    out.push_str("        port: 5404-5405/udp\n");
    out.push_str("        permanent: yes\n");
    out.push_str("        immediate: yes\n");
    out.push_str("        state: enabled\n\n");
    out.push_str("    - name: pcsd 포트 허용 (2224/tcp)\n");
    out.push_str("      ansible.posix.firewalld:\n");
    out.push_str("        zone: work\n");
    out.push_str("        port: 2224/tcp\n");
    out.push_str("        permanent: yes\n");
    out.push_str("        immediate: yes\n");
    out.push_str("        state: enabled\n\n");

    // 3. pcsd
    out.push_str("    - name: pcsd 서비스 시작 및 활성화\n");
    out.push_str("      ansible.builtin.systemd:\n");
    out.push_str("        name: pcsd\n");
    out.push_str("        state: started\n");
    out.push_str("        enabled: yes\n\n");

    // 4. hacluster 계정 (no_log 필수 — 두 태스크 모두)
    out.push_str("    - name: hacluster 비밀번호 해시 생성\n");
    out.push_str("      ansible.builtin.set_fact:\n");
    out.push_str("        hacluster_password_hash: \"{{ hacluster_password | password_hash('sha512') }}\"\n");
    out.push_str("      no_log: true\n\n");
    out.push_str("    - name: hacluster 계정 생성\n");
    out.push_str("      ansible.builtin.user:\n");
    out.push_str("        name: hacluster\n");
    out.push_str("        comment: \"pacemaker user\"\n");
    out.push_str("        uid: 189\n");
    out.push_str("        shell: /sbin/nologin\n");
    out.push_str("        home: /var/lib/hacluster\n");
    out.push_str("        group: haclient\n");
    out.push_str("        password: \"{{ hacluster_password_hash }}\"\n");
    out.push_str("      no_log: true\n\n");

    // 5. 멱등성 체크 — 대표 노드에서만
    out.push_str("    - name: 클러스터 존재 여부 확인\n");
    out.push_str("      ansible.builtin.command: pcs status\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      register: pcs_status\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    // 6. 클러스터가 아직 없을 때만 — 전부 대표 노드에서 실행
    out.push_str("    - name: 노드 인증 (클러스터 미존재 시)\n");
    out.push_str("      ansible.builtin.command: \"pcs host auth {{ ansible_play_batch | join(' ') }} -u hacluster -p {{ hacluster_password }}\"\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      when: pcs_status.rc != 0\n");
    out.push_str("      no_log: true\n\n");

    out.push_str("    - name: 클러스터 생성 (클러스터 미존재 시)\n");
    out.push_str(&format!(
        "      ansible.builtin.command: \"pcs cluster setup {} {{{{ ansible_play_batch | join(' ') }}}}\"\n",
        yaml_dquote_inner(&config.cluster.cluster_name)
    ));
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      when: pcs_status.rc != 0\n\n");

    out.push_str("    - name: 클러스터 시작 (클러스터 미존재 시)\n");
    out.push_str("      ansible.builtin.command: pcs cluster start --all\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      when: pcs_status.rc != 0\n\n");

    out.push_str("    - name: 클러스터 부팅시 활성화 (클러스터 미존재 시)\n");
    out.push_str("      ansible.builtin.command: pcs cluster enable --all\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      when: pcs_status.rc != 0\n\n");

    // 7. 클러스터 속성 (매번 재실행해도 안전 — pcs property set은 이미 멱등)
    out.push_str("    - name: no-quorum-policy 설정\n");
    out.push_str(&format!(
        "      ansible.builtin.command: \"pcs property set no-quorum-policy={}\"\n",
        yaml_dquote_inner(&config.cluster.no_quorum_policy)
    ));
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      changed_when: true\n\n");

    out.push_str("    - name: stonith-enabled 설정\n");
    out.push_str(&format!(
        "      ansible.builtin.command: \"pcs property set stonith-enabled={}\"\n",
        config.cluster.stonith_enabled
    ));
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      changed_when: true\n");

    Some(out)
}

/// PLAN.md A.5 — STONITH 리소스 멱등 등록.
/// `pcs stonith status`를 한 번만 조회해서 이미 있는 리소스는 건너뛴다
/// (A.3와 동일한 사전-캡처 방식). IPMI 정보가 없는 항목은 파싱 단계에서
/// 이미 걸러진다(`generate()`의 stonith_devices_json 파싱 참고).
fn build_stonith_play(config: &PacemakerConfig, hosts: &str, user: &str, key: &str) -> Option<String> {
    if config.stonith_devices.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("- name: STONITH(fencing) 리소스 등록\n");
    out.push_str(&format!("  hosts: {}\n", hosts));
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));
    out.push_str("  tasks:\n");

    out.push_str("    - name: 기존 STONITH 리소스 조회\n");
    out.push_str("      ansible.builtin.command: pcs stonith status\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      register: existing_stonith\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    for dev in &config.stonith_devices {
        let id = format!("stonith-ipmi-{}", sanitize_stonith_id(&dev.node));
        out.push_str(&format!("    - name: STONITH 생성 ({})\n", dev.node));
        out.push_str("      ansible.builtin.command: >\n");
        out.push_str(&format!("        pcs stonith create {} fence_ipmilan\n", id));
        out.push_str(&format!("        pcmk_host_list={}\n", dev.node));
        out.push_str(&format!(
            "        ip={} user={} password={}\n",
            dev.ipmi_ip, dev.ipmi_user, dev.ipmi_password
        ));
        out.push_str("        lanplus=1 power_wait=5 pcmk_reboot_timeout=300\n");
        out.push_str("        pcmk_monitor_timeout=60 pcmk_reboot_action=reboot\n");
        out.push_str("      run_once: true\n");
        out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
        out.push_str(&format!(
            "      when: {}\n",
            yaml_squote(&format!("existing_stonith.stdout is not search(\"{}\")", id))
        ));
        out.push_str("      no_log: true\n\n");
    }

    // 마지막 빈 줄 제거
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');

    Some(out)
}

/// PLAN.md A.3 — 리소스/제약조건 생성의 실제 멱등 배포.
///
/// `R11.pacemaker_drbd_resources.yml`이 실제로 쓰는 패턴 그대로: 리소스마다
/// 개별로 `pcs resource config <id>`를 부르는 대신, 맨 앞에서 딱 두 번만
/// (`pcs resource config`, `pcs constraint --full`) 상태를 통째로 캡처한
/// 뒤, 각 생성 태스크를 그 캡처된 텍스트에 대한 `is not search(...)`로
/// 가드한다. 리소스 그룹(`pcs resource group add`)은 pcs 자체가 이미
/// 멱등하게 처리하므로 별도 가드를 붙이지 않는다.
///
/// ⚠️ `pcs resource config`/`pcs constraint --full`의 정확한 출력 문자열
/// 포맷은 pcs 버전마다 다를 수 있다 — 아래 검색 패턴은 R11이 실제 운영
/// 클러스터에서 확인한 문자열을 그대로 따르지만, 이 앱은 그 출력을 직접
/// 검증할 수 없으므로 실제 클러스터에서 첫 실행 시 확인이 필요하다
/// (PLAN.md 검증/테스트 계획 참고).
fn build_resources_play(config: &PacemakerConfig, hosts: &str, user: &str, key: &str) -> String {
    let mut out = String::new();
    out.push_str("- name: Pacemaker 리소스 및 제약조건 구성\n");
    out.push_str(&format!("  hosts: {}\n", hosts));
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));
    out.push_str("  tasks:\n");

    out.push_str("    - name: 기존 pcs resource 구성 조회\n");
    out.push_str("      ansible.builtin.command: pcs resource config\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      register: existing_resource_config\n");
    out.push_str("      changed_when: false\n\n");

    out.push_str("    - name: 기존 pcs constraint 구성 조회\n");
    out.push_str("      ansible.builtin.command: pcs constraint --full\n");
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str("      register: existing_constraints\n");
    out.push_str("      changed_when: false\n\n");

    // clone-max는 generate_drbd_resource_cmds와 동일한 계산 로직 (PLAN.md A.2)
    let node_count = if config.cluster.nodes.is_empty() { 3 } else { config.cluster.nodes.len() };

    for drbd in &config.drbd_resources {
        push_guarded_resource_task(
            &mut out,
            &format!("DRBD Promotable Clone 생성 ({})", drbd.resource_name),
            &generate_drbd_resource_cmds(drbd, node_count),
            &drbd.resource_name,
        );
    }

    for fs in &config.fs_resources {
        push_guarded_resource_task(
            &mut out,
            &format!("Filesystem 리소스 생성 ({})", fs.resource_name),
            &generate_fs_resource_cmds(fs),
            &fs.resource_name,
        );
    }

    for svc in &config.systemd_resources {
        push_guarded_resource_task(
            &mut out,
            &format!("Systemd 리소스 생성 ({})", svc.resource_name),
            &generate_systemd_resource_cmds(svc),
            &svc.resource_name,
        );
    }

    // 리소스 그룹: pcs resource group add는 이미 멱등이라 가드 불필요
    for grp in &config.resource_groups {
        out.push_str(&format!("    - name: 리소스 그룹 구성 ({})\n", grp.group_name));
        out.push_str(&format!("      ansible.builtin.command: {}\n", yaml_dquote(&generate_resource_group_cmd(grp))));
        out.push_str("      run_once: true\n");
        out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
        out.push_str("      changed_when: true\n\n");
    }

    for ord in &config.order_constraints {
        let pattern = format!(
            "{} resource '{}' then {} resource '{}'",
            ord.first_action, ord.first, ord.then_action, ord.then
        );
        push_guarded_constraint_task(
            &mut out,
            &format!("Order 제약조건 ({} → {})", ord.first, ord.then),
            &generate_order_constraint(ord),
            &pattern,
        );
    }

    for col in &config.colocation_constraints {
        // R11 실측 패턴은 항상 역할(Started/Promoted)을 명시하는 경우만
        // 확인됨 — 역할이 없는 경우의 정확한 pcs 출력 포맷은 검증 못 함
        // (주석 참고).
        let rsc_role_prefix = col.rsc_role.as_deref().map(|r| format!("{} ", r)).unwrap_or_default();
        let pattern = match col.with_rsc_role.as_deref() {
            Some(role) => format!(
                "{}resource '{}' with {} resource '{}'",
                rsc_role_prefix, col.rsc, role, col.with_rsc
            ),
            None => format!("resource '{}' with resource '{}'", col.rsc, col.with_rsc),
        };
        push_guarded_constraint_task(
            &mut out,
            &format!("Colocation 제약조건 ({} with {})", col.rsc, col.with_rsc),
            &generate_colocation_constraint(col),
            &pattern,
        );
    }

    for loc in &config.location_constraints {
        let pattern = format!("resource '{}' prefers node '{}'", loc.rsc, loc.node);
        push_guarded_constraint_task(
            &mut out,
            &format!("Location 제약조건 ({} → {})", loc.rsc, loc.node),
            &generate_location_constraint(loc),
            &pattern,
        );
    }

    // 마지막 빈 줄 제거
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');

    out
}

/// 여러 줄(`\` 이어붙임 포함)로 된 리소스 생성 명령을 하나의 가드된
/// Ansible 태스크로 출력한다. 리소스 id가 `existing_resource_config`에
/// 이미 있으면 건너뛴다.
fn push_guarded_resource_task(out: &mut String, name: &str, cmd_lines: &[String], resource_id: &str) {
    out.push_str(&format!("    - name: {}\n", name));
    out.push_str("      ansible.builtin.shell: |\n");
    for line in cmd_lines {
        out.push_str(&format!("        {}\n", line));
    }
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str(&format!(
        "      when: {}\n",
        yaml_squote(&format!("existing_resource_config.stdout is not search(\"Resource: {} (\")", resource_id))
    ));
    out.push_str("      changed_when: true\n\n");
}

/// 단일 pcs constraint 명령을 가드된 Ansible 태스크로 출력한다.
fn push_guarded_constraint_task(out: &mut String, name: &str, cmd: &str, search_pattern: &str) {
    out.push_str(&format!("    - name: {}\n", name));
    out.push_str(&format!("      ansible.builtin.command: {}\n", yaml_dquote(cmd)));
    out.push_str("      run_once: true\n");
    out.push_str("      delegate_to: \"{{ ansible_play_batch | first }}\"\n");
    out.push_str(&format!(
        "      when: {}\n",
        yaml_squote(&format!("existing_constraints.stdout is not search(\"{}\")", search_pattern))
    ));
    out.push_str("      changed_when: true\n\n");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pacemaker::{ClusterNode, DrbdPacemakerResource};

    fn sample_form() -> PacemakerFormData {
        PacemakerFormData {
            cluster_name: "ha-cluster".to_string(),
            nodes_json: None,
            stonith_enabled: None,
            no_quorum_policy: "stop".to_string(),
            migration_threshold: None,
            failure_timeout: None,
            drbd_resources_json: None,
            systemd_resources_json: None,
            resource_groups_json: None,
            stonith_devices_json: None,
            auto_constraints: None,
            order_constraints_json: None,
            colocation_constraints_json: None,
            location_constraints_json: None,
            ansible_hosts: Some("container_service".to_string()),
            ansible_user: Some("deploy".to_string()),
            ansible_ssh_key: Some("~/.ssh/deploy_key".to_string()),
        }
    }

    fn config_with_nodes(n: usize) -> PacemakerConfig {
        let mut config = PacemakerConfig::default();
        config.cluster.nodes = (0..n)
            .map(|i| ClusterNode { name: format!("node{}", i + 1), id: (i + 1) as u32 })
            .collect();
        config.cluster.cluster_name = "ha-cluster".to_string();
        config
    }

    #[test]
    fn bootstrap_play_omitted_when_no_nodes() {
        let form = sample_form();
        let config = PacemakerConfig::default(); // nodes 비어 있음
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);
        assert!(!playbook.contains("클러스터 부트스트랩"));
    }

    #[test]
    fn bootstrap_play_included_with_versionlock_and_firewall_when_nodes_present() {
        let form = sample_form();
        let config = config_with_nodes(3);
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);

        assert!(playbook.contains("클러스터 부트스트랩"));
        assert!(playbook.contains("dnf config-manager --set-enabled crb"));
        assert!(playbook.contains("dnf config-manager --set-enabled highavailability"));
        assert!(playbook.contains("dnf versionlock add"));
        assert!(playbook.contains("port: 5404-5405/udp"));
        assert!(playbook.contains("port: 2224/tcp"));
        assert!(playbook.contains("uid: 189"));
        assert!(playbook.contains("pcs cluster setup ha-cluster"));
        assert!(playbook.contains("ansible_play_batch | join(' ')"));
        assert!(playbook.contains("ansible_play_batch | first"));
        // groups['container'] 같은 리터럴 그룹명 관용구를 쓰면 안 됨 (A.4 주의사항)
        assert!(!playbook.contains("groups['container']"));
    }

    #[test]
    fn stonith_play_omitted_when_no_devices() {
        let form = sample_form();
        let config = config_with_nodes(3);
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);
        assert!(!playbook.contains("STONITH(fencing) 리소스 등록"));
    }

    #[test]
    fn stonith_play_guards_on_existing_stonith_status() {
        let form = sample_form();
        let mut config = config_with_nodes(3);
        config.stonith_devices.push(StonithDevice {
            node: "node1".to_string(),
            ipmi_ip: "10.0.0.1".to_string(),
            ipmi_user: "admin".to_string(),
            ipmi_password: "secret".to_string(),
            extra_opts: vec![],
        });
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);

        assert!(playbook.contains("pcs stonith status"));
        assert!(playbook.contains("register: existing_stonith"));
        assert!(playbook.contains("pcs stonith create stonith-ipmi-node1 fence_ipmilan"));
        assert!(playbook.contains("ip=10.0.0.1 user=admin password=secret"));
        assert!(playbook.contains(
            "when: 'existing_stonith.stdout is not search(\"stonith-ipmi-node1\")'"
        ));
    }

    #[test]
    fn resources_play_precaptures_state_and_guards_each_item() {
        let form = sample_form();
        let mut config = config_with_nodes(3);
        config.drbd_resources.push(DrbdPacemakerResource {
            resource_name: "drbd-r0".to_string(),
            ..DrbdPacemakerResource::default()
        });
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);

        assert!(playbook.contains("pcs resource config"));
        assert!(playbook.contains("register: existing_resource_config"));
        assert!(playbook.contains("pcs constraint --full"));
        assert!(playbook.contains("register: existing_constraints"));
        assert!(playbook.contains(
            "when: 'existing_resource_config.stdout is not search(\"Resource: drbd-r0 (\")'"
        ));
    }

    #[test]
    fn order_and_colocation_constraints_are_individually_guarded() {
        let form = sample_form();
        let mut config = config_with_nodes(3);
        config.order_constraints.push(OrderConstraint {
            id: "ord-1".to_string(),
            first: "drbd-r0-clone".to_string(),
            first_action: "promote".to_string(),
            then: "fs-r0".to_string(),
            then_action: "start".to_string(),
            kind: "Mandatory".to_string(),
        });
        config.colocation_constraints.push(ColocationConstraint {
            id: "col-1".to_string(),
            rsc: "fs-r0".to_string(),
            rsc_role: None,
            with_rsc: "drbd-r0-clone".to_string(),
            with_rsc_role: Some("Promoted".to_string()),
            score: "INFINITY".to_string(),
        });
        let playbook = generate_pacemaker_ansible_playbook(&form, &config);

        assert!(playbook.contains(
            "when: 'existing_constraints.stdout is not search(\"promote resource ''drbd-r0-clone'' then start resource ''fs-r0''\")'"
        ));
        assert!(playbook.contains(
            "when: 'existing_constraints.stdout is not search(\"resource ''fs-r0'' with Promoted resource ''drbd-r0-clone''\")'"
        ));
    }
}


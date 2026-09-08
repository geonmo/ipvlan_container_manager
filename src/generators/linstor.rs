use crate::models::linstor::LinstorConfig;

/// `ansible-galaxy collection install`용 requirements.yml — 컬렉션이 아직
/// Ansible Galaxy에 게시되지 않아 git 소스로 설치해야 한다 (README 확인).
/// 리소스 설정과 무관한 고정 내용이라 상수로 둔다.
pub fn generate_linstor_requirements_yml() -> String {
    r#"---
# ansible-galaxy collection install -r requirements.yml
# LINBIT이 아직 Ansible Galaxy에 게시하지 않아 git 소스로 설치한다.
collections:
  - name: linbit.common
    source: https://github.com/LINBIT/ansible-common-collection.git
    type: git
  - name: linbit.drbd
    source: https://github.com/LINBIT/ansible-drbd-collection.git
    type: git
  - name: linbit.drbd_reactor
    source: https://github.com/LINBIT/ansible-drbd_reactor-collection.git
    type: git
  - name: linbit.linstor
    source: https://github.com/LINBIT/ansible-linstor-collection.git
    type: git
"#
    .to_string()
}

/// LINSTOR 스토리지 풀 하나를 `linstor_storage_pools` 항목 YAML로 변환한다.
/// 필드 이름은 `ansible-linstor-collection`의
/// `roles/storage_pool/tasks/main.yml` 실제 소스를 확인해 그대로 맞췄다
/// (vg/vg_thinpool/zpool/file_path/physical_devices).
fn storage_pool_yaml(pool: &crate::models::linstor::LinstorStoragePool) -> String {
    let mut out = String::new();
    out.push_str(&format!("      - name: {}\n", pool.name));
    out.push_str(&format!("        type: {}\n", pool.pool_type));
    if let Some(vg) = &pool.vg {
        if !vg.is_empty() {
            out.push_str(&format!("        vg: {}\n", vg));
        }
    }
    if let Some(vgtp) = &pool.vg_thinpool {
        if !vgtp.is_empty() {
            out.push_str(&format!("        vg_thinpool: {}\n", vgtp));
        }
    }
    if let Some(zpool) = &pool.zpool {
        if !zpool.is_empty() {
            out.push_str(&format!("        zpool: {}\n", zpool));
        }
    }
    if let Some(fp) = &pool.file_path {
        if !fp.is_empty() {
            out.push_str(&format!("        file_path: {}\n", fp));
        }
    }
    if !pool.physical_devices.is_empty() {
        out.push_str("        physical_devices:\n");
        for dev in &pool.physical_devices {
            out.push_str(&format!("          - {}\n", dev));
        }
    }
    if !pool.nodes.is_empty() {
        out.push_str("        nodes:\n");
        for n in &pool.nodes {
            out.push_str(&format!("          - {}\n", n));
        }
    }
    out
}

/// LINSTOR 전용 Ansible 인벤토리 (YAML) 생성.
///
/// `ansible-linstor-collection` README의 "Required inventory groups"를
/// 그대로 따른다: `linstor_controllers`/`linstor_satellites` 호스트
/// 그룹과 그 둘을 합친 `linstor_cluster` 그룹(각 role/`cluster_init`이
/// 이 이름들로 대상을 정한다). 노드는 컨트롤러와 새틀라이트에 동시에
/// 속할 수 있다(결합 노드) — 이 앱은 중복을 제거하지 않는다.
pub fn generate_linstor_inventory(
    config: &LinstorConfig,
    node_ips: &std::collections::HashMap<String, String>,
    ansible_user: &str,
    ansible_ssh_key: &str,
) -> String {
    let mut out = String::new();
    out.push_str("all:\n");
    out.push_str("  children:\n");

    out.push_str("    linstor_controllers:\n");
    out.push_str("      hosts:\n");
    for node in &config.controller_nodes {
        out.push_str(&format!("        {}:\n", node));
        let ip = node_ips.get(node).cloned().unwrap_or_else(|| node.clone());
        out.push_str(&format!("          ansible_host: {}\n", ip));
    }

    out.push_str("    linstor_satellites:\n");
    out.push_str("      hosts:\n");
    for node in &config.satellite_nodes {
        out.push_str(&format!("        {}:\n", node));
        let ip = node_ips.get(node).cloned().unwrap_or_else(|| node.clone());
        out.push_str(&format!("          ansible_host: {}\n", ip));
    }

    out.push_str("    linstor_cluster:\n");
    out.push_str("      children:\n");
    out.push_str("        linstor_controllers: {}\n");
    out.push_str("        linstor_satellites: {}\n");

    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_user: {}\n", ansible_user));
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", ansible_ssh_key));
    out.push_str("    ansible_host_key_checking: false\n");

    if config.deploy_storage && !config.storage_pools.is_empty() {
        out.push_str("    linstor_storage_pools:\n");
        for pool in &config.storage_pools {
            out.push_str(&storage_pool_yaml(pool));
        }
    }

    out
}

/// LINSTOR 배포 Ansible 플레이북 생성 (PLAN.md D.2).
///
/// 3개 play로 구성:
/// 1. `rpm_dir`(gsdc-linbit-build 산출물)을 클러스터 전 노드에 로컬
///    설치 — `cluster_init_repo_access: none`으로 돌릴 것이므로 role이
///    호출하는 `ansible.builtin.package`가 찾을 수 있도록 미리 깔아둔다.
/// 2. `linbit.linstor.cluster_init` role 한 번 호출로 설치~클러스터
///    등록까지 처리 (README의 "Minimal deployment" 예제 그대로).
/// 3. (선택) `resource_group`/`resource(mode: spawn)` 모듈로 실제 DRBD
///    리소스를 만든다 — README의 "Using LINSTOR modules" 예제 패턴.
pub fn generate_linstor_ansible_playbook(
    config: &LinstorConfig,
    ansible_user: &str,
    ansible_ssh_key: &str,
) -> String {
    let mut out = String::new();

    out.push_str("---\n");
    out.push_str("# LINSTOR 배포 플레이북 (PLAN.md D.2)\n");
    out.push_str("# 사전 준비: `ansible-galaxy collection install -r requirements.yml`\n");
    out.push_str("# (함께 생성된 requirements.yml 참고). rpm_dir에는 gsdc-linbit-build가\n");
    out.push_str("# 만든 linstor-*.rpm 파일들이 있어야 한다 — LINBIT 공식 EL9 저장소는\n");
    out.push_str("# 구독 고객 전용이라 이 앱은 이미 빌드된 RPM을 로컬 설치하는 것을\n");
    out.push_str("# 전제로 cluster_init_repo_access: none으로 role을 호출한다.\n\n");

    // ── Play 1: RPM 로컬 설치 ──────────────────────────────────────────
    out.push_str("- name: LINSTOR RPM 로컬 설치 (gsdc-linbit-build 산출물)\n");
    out.push_str("  hosts: linstor_cluster\n");
    out.push_str(&format!("  remote_user: {}\n", ansible_user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", ansible_ssh_key));
    out.push_str("  tasks:\n");
    out.push_str("    - name: 제어 노드의 로컬 RPM 목록 확인\n");
    out.push_str("      ansible.builtin.find:\n");
    out.push_str(&format!("        paths: \"{}\"\n", config.rpm_dir));
    out.push_str("        patterns: \"*.rpm\"\n");
    out.push_str("      delegate_to: localhost\n");
    out.push_str("      run_once: true\n");
    out.push_str("      register: _linstor_rpms\n\n");
    out.push_str("    - name: RPM 파일을 노드로 복사\n");
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str("        src: \"{{ item.path }}\"\n");
    out.push_str("        dest: \"/tmp/linstor-rpms/{{ item.path | basename }}\"\n");
    out.push_str("      loop: \"{{ _linstor_rpms.files }}\"\n");
    out.push_str("      loop_control:\n");
    out.push_str("        label: \"{{ item.path | basename }}\"\n\n");
    out.push_str("    - name: LINSTOR RPM 설치 (dnf, 로컬 파일 트랜잭션)\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: \"/tmp/linstor-rpms/*.rpm\"\n");
    out.push_str("        state: present\n");
    out.push_str("        disable_gpg_check: true\n\n");

    // ── Play 2: cluster_init ───────────────────────────────────────────
    out.push_str("- name: LINSTOR 클러스터 설치/초기화 (linbit.linstor.cluster_init)\n");
    out.push_str("  hosts: linstor_cluster\n");
    out.push_str("  any_errors_fatal: true\n");
    out.push_str(&format!("  remote_user: {}\n", ansible_user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", ansible_ssh_key));
    out.push_str("    # RPM은 위 play에서 이미 로컬 설치했으므로 repo 설정은 건너뛴다\n");
    out.push_str("    # (customer/public 저장소 모두 이 앱의 대상인 EL9에서는 무료로 접근 불가 — PLAN.md D 범위 확정 참고)\n");
    out.push_str("    cluster_init_repo_access: none\n");
    out.push_str(&format!("    cluster_init_deploy_storage: {}\n", config.deploy_storage));
    out.push_str(&format!("    cluster_init_ha_database: {}\n", config.ha_database));
    out.push_str(&format!("    cluster_init_token_auth: {}\n", config.token_auth));
    out.push_str("  tasks:\n");
    out.push_str("    - name: LINSTOR 설치 및 클러스터 등록\n");
    out.push_str("      ansible.builtin.import_role:\n");
    out.push_str("        name: linbit.linstor.cluster_init\n\n");

    // ── Play 3: 리소스 그룹/리소스 프로비저닝 (선택) ───────────────────
    if !config.resource_groups.is_empty() || !config.resources.is_empty() {
        out.push_str("- name: LINSTOR 리소스 그룹/리소스 프로비저닝\n");
        out.push_str("  hosts: localhost\n");
        out.push_str("  gather_facts: false\n");
        out.push_str("  environment:\n");
        out.push_str("    LS_CONTROLLERS: \"{{ lookup('linbit.linstor.controller_env') }}\"\n");
        out.push_str("  tasks:\n");
        for rg in &config.resource_groups {
            out.push_str(&format!("    - name: 리소스 그룹 생성 ({})\n", rg.name));
            out.push_str("      linbit.linstor.resource_group:\n");
            out.push_str(&format!("        name: {}\n", rg.name));
            out.push_str(&format!("        storage_pool: {}\n", rg.storage_pool));
            out.push_str(&format!("        place_count: {}\n\n", rg.place_count));
        }
        for res in &config.resources {
            out.push_str(&format!("    - name: 리소스 스폰 ({})\n", res.name));
            out.push_str("      linbit.linstor.resource:\n");
            out.push_str(&format!("        name: {}\n", res.name));
            out.push_str("        mode: spawn\n");
            out.push_str(&format!("        resource_group: {}\n", res.resource_group));
            out.push_str(&format!("        size: {}\n\n", res.size));
        }
    }

    // 마지막 빈 줄 정리
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::linstor::{LinstorResourceGroup, LinstorResourceSpawn, LinstorStoragePool};
    use std::collections::HashMap;

    fn sample_config() -> LinstorConfig {
        LinstorConfig {
            controller_nodes: vec!["node1".to_string()],
            satellite_nodes: vec!["node1".to_string(), "node2".to_string(), "node3".to_string()],
            rpm_dir: "/opt/linstor-rpms".to_string(),
            storage_pools: vec![LinstorStoragePool {
                name: "sp-lvmthin".to_string(),
                pool_type: "lvmthin".to_string(),
                vg: Some("vg_linstor".to_string()),
                vg_thinpool: Some("thinpool".to_string()),
                zpool: None,
                file_path: None,
                physical_devices: vec!["/dev/sdb".to_string()],
                nodes: vec!["node1".to_string(), "node2".to_string(), "node3".to_string()],
            }],
            resource_groups: vec![LinstorResourceGroup {
                name: "rg-app".to_string(),
                storage_pool: "sp-lvmthin".to_string(),
                place_count: 3,
            }],
            resources: vec![LinstorResourceSpawn {
                name: "res-app".to_string(),
                resource_group: "rg-app".to_string(),
                size: "100G".to_string(),
            }],
            deploy_storage: true,
            ha_database: false,
            token_auth: true,
        }
    }

    #[test]
    fn inventory_has_required_linstor_groups() {
        let config = sample_config();
        let mut ips = HashMap::new();
        ips.insert("node1".to_string(), "192.168.1.11".to_string());
        ips.insert("node2".to_string(), "192.168.1.12".to_string());
        ips.insert("node3".to_string(), "192.168.1.13".to_string());
        let inv = generate_linstor_inventory(&config, &ips, "deploy", "~/.ssh/id_rsa");

        assert!(inv.contains("linstor_controllers:"));
        assert!(inv.contains("linstor_satellites:"));
        assert!(inv.contains("linstor_cluster:"));
        assert!(inv.contains("linstor_storage_pools:"));
        assert!(inv.contains("vg_thinpool: thinpool"));
    }

    #[test]
    fn playbook_installs_rpms_before_cluster_init() {
        let config = sample_config();
        let pb = generate_linstor_ansible_playbook(&config, "deploy", "~/.ssh/id_rsa");

        let rpm_pos = pb.find("LINSTOR RPM 로컬 설치").unwrap();
        let init_pos = pb.find("linbit.linstor.cluster_init").unwrap();
        assert!(rpm_pos < init_pos);
        assert!(pb.contains("cluster_init_repo_access: none"));
        assert!(pb.contains("hosts: linstor_cluster"));
    }

    #[test]
    fn playbook_omits_resource_play_when_empty() {
        let mut config = sample_config();
        config.resource_groups.clear();
        config.resources.clear();
        let pb = generate_linstor_ansible_playbook(&config, "deploy", "~/.ssh/id_rsa");
        assert!(!pb.contains("리소스 그룹/리소스 프로비저닝"));
    }

    #[test]
    fn playbook_emits_resource_group_and_spawn_tasks() {
        let config = sample_config();
        let pb = generate_linstor_ansible_playbook(&config, "deploy", "~/.ssh/id_rsa");
        assert!(pb.contains("linbit.linstor.resource_group:"));
        assert!(pb.contains("place_count: 3"));
        assert!(pb.contains("mode: spawn"));
        assert!(pb.contains("size: 100G"));
    }
}

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

/// 노드 풀의 IP를 `ansible_host` 값으로 바꾼다.
///
/// 노드 풀의 `ip` 컬럼은 기본값이 빈 문자열이라 `HashMap::get`이
/// `Some("")`을 돌려줄 수 있다. 그대로 쓰면 `ansible_host: ` (빈 값)이
/// 되어 인벤토리가 깨지므로, 비어 있으면 hostname으로 되돌린다
/// (`routes/nodes.rs`의 write_ansible_inventory와 동일한 규칙).
fn resolve_ansible_host(node_ips: &std::collections::HashMap<String, String>, node: &str) -> String {
    node_ips
        .get(node)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(node)
        .to_string()
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
        let ip = resolve_ansible_host(node_ips, node);
        out.push_str(&format!("          ansible_host: {}\n", ip));
    }

    out.push_str("    linstor_satellites:\n");
    out.push_str("      hosts:\n");
    for node in &config.satellite_nodes {
        out.push_str(&format!("        {}:\n", node));
        let ip = resolve_ansible_host(node_ips, node);
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

    // ── Play 0: Pacemaker 관리 전제조건 ────────────────────────────────
    // 3노드 실클러스터 검증에서 이것들이 빠지면 각각 다른 증상으로 실패했다.
    // Pacemaker가 리소스를 관리하지 않는 구성이면 생략한다.
    if config.pacemaker_managed {
        out.push_str("- name: Pacemaker 관리 전제조건 (DRBD 모듈 / SELinux)\n");
        out.push_str("  hosts: linstor_cluster\n");
        out.push_str(&format!("  remote_user: {}\n", ansible_user));
        out.push_str("  become: yes\n");
        out.push_str("  # ansible_facts['selinux'] 가 필요하다\n");
        out.push_str("  gather_facts: yes\n");
        out.push_str("  vars:\n");
        out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", ansible_ssh_key));
        out.push_str("  tasks:\n");

        // (a) DRBD 커널 모듈
        out.push_str("    # linbit.drbd.drbd_install(satellite_install 의 meta 의존성)은\n");
        out.push_str("    # /sys/module/drbd/version 으로 설치 여부를 판단하는데, 이 경로는\n");
        out.push_str("    # 모듈이 **로드돼 있을 때만** 존재한다. 패키지만 깔려 있고 로드가\n");
        out.push_str("    # 안 돼 있으면 role 이 LINBIT 자체 패키지명(kmod-drbd)을 설치하려다\n");
        out.push_str("    # \"No package kmod-drbd available.\" 로 실패한다.\n");
        out.push_str("    - name: 부팅 시 DRBD 모듈 자동 로드 설정\n");
        out.push_str("      ansible.builtin.copy:\n");
        out.push_str("        content: \"drbd\\n\"\n");
        out.push_str("        dest: /etc/modules-load.d/drbd.conf\n");
        out.push_str("        mode: '0644'\n\n");
        out.push_str("    - name: DRBD 모듈 즉시 로드\n");
        out.push_str("      community.general.modprobe:\n");
        out.push_str("        name: drbd\n");
        out.push_str("        state: present\n\n");

        // (b) drbd-selinux
        out.push_str("    # SELinux Enforcing 에서 ocf:linbit:drbd RA 는 drbd_t 도메인으로\n");
        out.push_str("    # 실행된다. 이 정책 모듈이 없으면 drbdsetup 이 커널과 통신할\n");
        out.push_str("    # netlink_generic_socket 조차 만들지 못해 RA 가 리소스를 찾지 못한다.\n");
        out.push_str("    - name: drbd-selinux 설치 (SELinux 정책 모듈)\n");
        out.push_str("      ansible.builtin.dnf:\n");
        out.push_str("        name: drbd-selinux\n");
        out.push_str("        state: present\n");
        out.push_str("      when: ansible_facts['selinux']['status'] | default('disabled') != 'disabled'\n\n");

        // (c) /var/lib/linstor.d 라벨
        out.push_str("    # LINSTOR 는 .res 파일을 /var/lib/linstor.d/ 에 쓰는데 그건 var_lib_t\n");
        out.push_str("    # 로 라벨된다. DRBD 정책은 /etc/drbd.d(etc_t)만 읽도록 돼 있어서\n");
        out.push_str("    # drbd_t 가 읽지 못하고, 증상은 엉뚱하게 나온다:\n");
        out.push_str("    #   avc: denied { read } comm=\"drbdadm\" scontext=drbd_t tcontext=var_lib_t\n");
        out.push_str("    #   -> \"DRBD resource <name> not found in configuration file /etc/drbd.conf.\"\n");
        out.push_str("    # 셸에서 root 로 drbdadm 을 치면 unconfined_t 라 잘 되기 때문에\n");
        out.push_str("    # AVC 로그를 보지 않으면 원인을 찾기 어렵다.\n");
        out.push_str("    - name: /var/lib/linstor.d 를 etc_t 로 라벨링\n");
        out.push_str("      community.general.sefcontext:\n");
        out.push_str("        target: '/var/lib/linstor\\.d(/.*)?'\n");
        out.push_str("        setype: etc_t\n");
        out.push_str("        state: present\n");
        out.push_str("      when: ansible_facts['selinux']['status'] | default('disabled') != 'disabled'\n");
        out.push_str("      register: _linstor_fcontext\n\n");
        out.push_str("    - name: 기존 파일에 라벨 적용\n");
        out.push_str("      ansible.builtin.command:\n");
        out.push_str("        cmd: restorecon -RF /var/lib/linstor.d\n");
        out.push_str("      when:\n");
        out.push_str("        - ansible_facts['selinux']['status'] | default('disabled') != 'disabled'\n");
        out.push_str("        - _linstor_fcontext is changed\n");
        out.push_str("      changed_when: true\n\n");
    }

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
            out.push_str(&format!("        place_count: {}\n", rg.place_count));
            if config.pacemaker_managed {
                // Pacemaker 가 승격 시점을 통제하려면 커널의 자동 승격을 꺼야 한다.
                // 리소스 그룹에 걸면 그 그룹에서 만들어진 기존 리소스까지 전파된다.
                out.push_str("        drbd_options:\n");
                out.push_str("          resource:\n");
                out.push_str("            auto-promote: \"no\"\n");
            }
            out.push('\n');
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
            pacemaker_managed: true,
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

    /// 노드 풀의 `ip` 컬럼은 기본값이 빈 문자열이라 HashMap이 Some("")를
    /// 돌려줄 수 있다. 그대로 쓰면 `ansible_host: ` (빈 값)이 되어
    /// 인벤토리가 깨진다.
    #[test]
    fn inventory_falls_back_to_hostname_when_pool_ip_is_blank() {
        let config = sample_config();
        let mut ips = HashMap::new();
        ips.insert("node1".to_string(), String::new()); // 빈 IP
        ips.insert("node2".to_string(), "   ".to_string()); // 공백만
        ips.insert("node3".to_string(), "192.168.1.13".to_string());

        let inv = generate_linstor_inventory(&config, &ips, "deploy", "~/.ssh/id_rsa");

        assert!(inv.contains("ansible_host: node1"));
        assert!(inv.contains("ansible_host: node2"));
        assert!(inv.contains("ansible_host: 192.168.1.13"));
        // 값이 빈 ansible_host 줄이 있으면 안 된다.
        assert!(!inv.lines().any(|l| l.trim() == "ansible_host:"));

        let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&inv);
        assert!(parsed.is_ok(), "인벤토리 YAML 파싱 실패: {:?}", parsed.err());
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

    /// Pacemaker 가 ocf:linbit:drbd 로 리소스를 관리하려면 세 가지 전제조건이
    /// 필요하다. 3노드 실클러스터에서 각각이 빠졌을 때 서로 다른 증상으로
    /// 실패하는 것을 확인했고, 특히 SELinux 라벨 문제는 증상("resource not
    /// found in configuration file")이 원인과 동떨어져 보여 찾기 어렵다.
    #[test]
    fn pacemaker_managed_emits_all_three_prerequisites() {
        let pb = generate_linstor_ansible_playbook(&sample_config(), "deploy", "~/.ssh/id_rsa");

        // 1. DRBD 커널 모듈 로드 (drbd_install 이 /sys/module/drbd/version 을 본다)
        assert!(pb.contains("/etc/modules-load.d/drbd.conf"));
        assert!(pb.contains("community.general.modprobe"));
        // 2. SELinux 정책 모듈
        assert!(pb.contains("name: drbd-selinux"));
        // 3. LINSTOR .res 디렉터리 라벨
        assert!(pb.contains("community.general.sefcontext"));
        assert!(pb.contains("setype: etc_t"));
        assert!(pb.contains("restorecon -RF /var/lib/linstor.d"));
        // 4. 커널 자동 승격 비활성화
        assert!(pb.contains("auto-promote: \"no\""));

        // 전제조건 play 는 RPM 설치보다 앞서야 한다 (satellite_install 이
        // drbd_install 을 meta 의존성으로 끌어오기 때문)
        let prereq = pb.find("Pacemaker 관리 전제조건").expect("전제조건 play 없음");
        let rpm = pb.find("LINSTOR RPM 로컬 설치").expect("RPM play 없음");
        assert!(prereq < rpm, "전제조건 play 가 RPM 설치보다 앞에 와야 한다");
    }

    #[test]
    fn pacemaker_managed_off_omits_prerequisites() {
        let mut config = sample_config();
        config.pacemaker_managed = false;
        let pb = generate_linstor_ansible_playbook(&config, "deploy", "~/.ssh/id_rsa");

        assert!(!pb.contains("Pacemaker 관리 전제조건"));
        assert!(!pb.contains("drbd-selinux"));
        assert!(!pb.contains("sefcontext"));
        // LINSTOR 단독 운용이면 커널 자동 승격을 끄지 않는다
        assert!(!pb.contains("auto-promote"));
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

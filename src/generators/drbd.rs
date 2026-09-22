use crate::models::drbd::{AnsibleInventory, DrbdResource, ScannedDrbdNode, ScannedDrbdResource};
use anyhow::Result;

// ─────────────────────────────────────────────────────────────
// .res 파일 생성
// ─────────────────────────────────────────────────────────────

/// `on-io-error` 값을 DRBD 9가 실제로 받는 값으로 정규화한다.
///
/// drbdadm 9.34 실측 허용값은 `pass_on | call-local-io-error | detach`
/// 뿐이다. 이 앱의 폼은 예전에 `passthrough`(기본값!)와 `panic`을
/// 제시했는데 둘 다 DRBD 9 파서가 거부해서, **기본 설정 그대로 만든
/// .res 파일조차 `drbdadm`이 로드하지 못했다**. UI 선택지는 고쳤지만
/// 이미 DB에 저장된 리소스와 브라우저 localStorage에는 옛 값이 남아
/// 있으므로, 의미가 가장 가까운 유효값으로 옮겨 준다.
fn normalize_on_io_error(value: &str) -> &'static str {
    match value.trim() {
        "pass_on" => "pass_on",
        "call-local-io-error" => "call-local-io-error",
        "detach" => "detach",
        // 레거시 값 이전
        "passthrough" => "pass_on",
        "panic" => "call-local-io-error",
        _ => "detach",
    }
}

/// `after-sb-1pri` 값 정규화. 허용값은
/// `disconnect | consensus | discard-secondary | call-pri-lost-after-sb |
/// violently-as0p` (drbdadm 9.34 실측)로, 폼이 제시하던 `call-pri-lost`는
/// 유효한 이름이 아니다(정확한 이름은 `call-pri-lost-after-sb`).
fn normalize_after_sb_1pri(value: &str) -> &'static str {
    match value.trim() {
        "disconnect" => "disconnect",
        "consensus" => "consensus",
        "discard-secondary" => "discard-secondary",
        "call-pri-lost-after-sb" => "call-pri-lost-after-sb",
        "violently-as0p" => "violently-as0p",
        // 레거시 값 이전
        "call-pri-lost" => "call-pri-lost-after-sb",
        _ => "discard-secondary",
    }
}

pub fn generate_res_file(resource: &DrbdResource) -> String {
    let mut out = String::new();

    out.push_str(&format!("resource {} {{\n", resource.resource_name));
    out.push_str(&format!("    protocol {};\n\n", resource.protocol));

    // options 블록
    out.push_str("    options {\n");
    out.push_str("        quorum majority;\n");
    out.push_str("        on-no-quorum io-error;\n");
    out.push_str("    }\n\n");

    // startup 블록 — 폼의 wfc-timeout / degr-wfc-timeout 값을 반영한다.
    //
    // DRBD 9 man page(drbd.conf-9.0) 기준 `startup` 섹션의 유효 키워드는
    // wfc-timeout / degr-wfc-timeout / outdated-wfc-timeout /
    // stacked-timeouts / wait-after-sb 뿐이며, 이 값들은 **부팅 시 drbd
    // init 스크립트**의 대기 동작만 바꾼다 ("They have no effect once the
    // system is up and running"). Pacemaker가 ocf:linbit:drbd로 리소스를
    // 올리는 경로에는 영향이 없지만, 폼이 입력을 받는 이상 생성물에
    // 그대로 나타나야 한다 (이전에는 값을 받아 DB에 저장만 하고 .res에는
    // 전혀 쓰지 않아 사용자가 바꿔도 결과가 동일했다).
    //
    // `become-primary-on`은 여기 넣지 않는다 — DRBD 9의 drbdadm(9.34
    // 실측)은 이 키워드를 에러 없이 조용히 버리고, 애초에 Primary 승격은
    // Pacemaker promotable clone이 결정한다. 선호 노드는 pacemaker 탭의
    // location 제약조건(`prefers <node>=200`)으로 지정한다.
    out.push_str("    startup {\n");
    out.push_str(&format!(
        "        wfc-timeout      {};\n",
        resource.startup_options.wfc_timeout
    ));
    out.push_str(&format!(
        "        degr-wfc-timeout {};\n",
        resource.startup_options.degr_wfc_timeout
    ));
    out.push_str("    }\n\n");

    // disk 블록 (on-io-error만, fencing은 net으로)
    out.push_str("    disk {\n");
    out.push_str(&format!(
        "        on-io-error {};\n",
        normalize_on_io_error(&resource.disk_options.on_io_error)
    ));
    out.push_str("    }\n\n");

    // net 블록 (fencing 포함)
    out.push_str("    net {\n");
    out.push_str(&format!(
        "        fencing {};\n",
        resource.disk_options.fencing
    ));
    out.push_str("        max-buffers 8000;\n");
    out.push_str("        max-epoch-size 8000;\n");
    if resource.net_options.allow_two_primaries {
        out.push_str("        allow-two-primaries yes;\n");
    }
    out.push_str(&format!(
        "        after-sb-0pri {};\n",
        resource.net_options.after_sb_0pri
    ));
    out.push_str(&format!(
        "        after-sb-1pri {};\n",
        normalize_after_sb_1pri(&resource.net_options.after_sb_1pri)
    ));
    if resource.net_options.after_sb_2pri != "disconnect" || resource.net_options.allow_two_primaries {
        out.push_str(&format!(
            "        after-sb-2pri {};\n",
            resource.net_options.after_sb_2pri
        ));
    }
    out.push_str("    }\n\n");

    // on <node> sections (node-id 포함)
    out.push_str("    # node-id values are assigned 0, 1, 2 ... in inventory order.\n");
    for (idx, node) in resource.nodes.iter().enumerate() {
        let disk_path = node.effective_disk(&resource.resource_name);
        out.push_str(&format!("    on {} {{\n", node.hostname));
        out.push_str(&format!("        node-id   {};\n", idx));
        out.push_str(&format!(
            "        device    /dev/drbd{};\n",
            resource.minor
        ));
        out.push_str(&format!("        disk      {};\n", disk_path));
        out.push_str(&format!(
            "        address   {}:{};\n",
            node.ip, node.port
        ));
        out.push_str(&format!("        meta-disk {};\n", node.meta_disk));
        out.push_str("    }\n");
    }

    // 3개 이상 노드: DRBD 9 connection-mesh 섹션
    if resource.nodes.len() >= 2 {
        let host_list = resource.nodes
            .iter()
            .map(|n| n.hostname.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str("\n    connection-mesh {\n");
        out.push_str(&format!("        hosts {} ;\n", host_list));
        out.push_str("    }\n");
    }

    out.push_str("}\n");
    out
}

// ─────────────────────────────────────────────────────────────
// global_common.conf 생성 (fence-peer 핸들러)
// ─────────────────────────────────────────────────────────────

/// `/etc/drbd.d/global_common.conf` 내용 생성.
///
/// `net { fencing resource-only; }` 정책은 피어 단절 시 fence-peer
/// 핸들러가 실행되어야만 promote가 진행된다. 이 핸들러는 `.res` 파일이
/// 아니라 노드별 `global_common.conf`의 `handlers {}` 블록에 등록해야
/// 하며, 없으면 promote가 응답 없이 hang 되다 op timeout으로 실패한다.
/// 핸들러 스크립트 자체는 `drbd9x-utils` 패키지에 이미 포함되어 있다.
pub fn generate_global_common_conf() -> String {
    let mut out = String::new();
    out.push_str("global {\n");
    out.push_str("    usage-count yes;\n");
    out.push_str("    udev-always-use-vnr;\n");
    out.push_str("}\n");
    out.push_str("common {\n");
    out.push_str("    handlers {\n");
    out.push_str("        fence-peer \"/usr/lib/drbd/crm-fence-peer.9.sh\";\n");
    out.push_str("        after-resync-target \"/usr/lib/drbd/crm-unfence-peer.9.sh\";\n");
    out.push_str("    }\n");
    out.push_str("    startup {\n");
    out.push_str("    }\n");
    out.push_str("    options {\n");
    out.push_str("    }\n");
    out.push_str("    disk {\n");
    out.push_str("    }\n");
    out.push_str("    net {\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

// ─────────────────────────────────────────────────────────────
// Ansible 인벤토리(YAML) 생성
// ─────────────────────────────────────────────────────────────

pub fn generate_ansible_inventory(inventory: &AnsibleInventory) -> Result<String> {
    let mut map = serde_yaml::Mapping::new();
    let mut all = serde_yaml::Mapping::new();
    let mut hosts = serde_yaml::Mapping::new();

    for node in &inventory.nodes {
        let mut host_vars = serde_yaml::Mapping::new();
        host_vars.insert(
            serde_yaml::Value::String("ansible_host".to_string()),
            serde_yaml::Value::String(node.ip.clone()),
        );
        hosts.insert(
            serde_yaml::Value::String(node.hostname.clone()),
            serde_yaml::Value::Mapping(host_vars),
        );
    }

    let mut vars = serde_yaml::Mapping::new();
    vars.insert(
        serde_yaml::Value::String("ansible_user".to_string()),
        serde_yaml::Value::String(inventory.ansible_user.clone()),
    );
    vars.insert(
        serde_yaml::Value::String("ansible_ssh_private_key_file".to_string()),
        serde_yaml::Value::String(inventory.ansible_ssh_private_key_file.clone()),
    );
    vars.insert(
        serde_yaml::Value::String("ansible_become".to_string()),
        serde_yaml::Value::Bool(inventory.r#become),
    );

    all.insert(
        serde_yaml::Value::String("hosts".to_string()),
        serde_yaml::Value::Mapping(hosts),
    );
    all.insert(
        serde_yaml::Value::String("vars".to_string()),
        serde_yaml::Value::Mapping(vars),
    );
    map.insert(
        serde_yaml::Value::String("all".to_string()),
        serde_yaml::Value::Mapping(all),
    );

    Ok(serde_yaml::to_string(&serde_yaml::Value::Mapping(map))?)
}

// ─────────────────────────────────────────────────────────────
// Ansible 플레이북 생성 (LVM + DRBD 통합)
// ─────────────────────────────────────────────────────────────

pub fn generate_ansible_playbook(resource: &DrbdResource, inventory: &AnsibleInventory) -> String {
    // LV를 실제로 만들 수 있는 노드만 추린다. VG 이름이 비어 있으면 lvol
    // 태스크를 만들 수 없으므로 여기서 걸러야 `drbd_resources`가 빈 키로
    // 남아 `loop`가 null을 받는 일이 없다.
    let lvm_nodes: Vec<&crate::models::drbd::DrbdNode> = resource
        .nodes
        .iter()
        .filter(|n| n.disk_type == "lvm" && !n.lvm_vg.is_empty())
        .collect();
    let has_lvm = !lvm_nodes.is_empty();
    let res_name = &resource.resource_name;

    // 포트 범위 문자열 (7788-7799 기준, 실제 사용 포트 기반으로 범위 계산)
    let ports: Vec<u16> = {
        let mut p: Vec<u16> = resource.nodes.iter().map(|n| n.port).collect();
        p.sort_unstable();
        p.dedup();
        p
    };
    let port_range = if ports.len() == 1 {
        format!("{}/tcp", ports[0])
    } else {
        format!("{}-{}/tcp", ports.first().unwrap(), ports.last().unwrap())
    };

    let user = &inventory.ansible_user;
    let key  = &inventory.ansible_ssh_private_key_file;
    let first_node = resource.nodes.first().map(|n| n.hostname.as_str()).unwrap_or("node1");

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("# Deployment playbook for DRBD resource '{}' (auto-generated)\n\n", res_name));

    // ── Play 1: DRBD 패키지 설치 (hosts: drbd) ─────────────────
    out.push_str(&format!("- name: Install DRBD packages\n"));
    out.push_str("  hosts: all\n");
    out.push_str("  become: true\n");
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));
    out.push_str("    kernel_packages:\n");
    out.push_str("      - kernel\n");
    out.push_str("      - kernel-devel\n");
    out.push_str("    drbd_packages:\n");
    out.push_str("      - drbd9x-utils\n");
    out.push_str("      - kmod-drbd9x\n");
    out.push_str("      - drbd-selinux\n");
    out.push_str("  tasks:\n");

    out.push_str("    - name: 1. Import the ELRepo GPG key\n");
    out.push_str("      ansible.builtin.rpm_key:\n");
    out.push_str("        state: present\n");
    out.push_str("        key: https://www.elrepo.org/RPM-GPG-KEY-elrepo.org\n\n");

    out.push_str("    - name: 2. Install the ELRepo repository (AlmaLinux 9)\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: https://www.elrepo.org/elrepo-release-9.el9.elrepo.noarch.rpm\n");
    out.push_str("        state: present\n");
    out.push_str("        disable_gpg_check: yes\n\n");

    out.push_str("    - name: 3. Install dnf-plugin-versionlock\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: python3-dnf-plugin-versionlock\n");
    out.push_str("        state: present\n\n");

    out.push_str("    - name: 4. Update kernel packages\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: \"{{ kernel_packages }}\"\n");
    out.push_str("        state: latest\n");
    out.push_str("        enablerepo: baseos,appstream\n");
    out.push_str("      register: kernel_update_result\n\n");

    out.push_str("    - name: 5. Install the DRBD packages\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: \"{{ drbd_packages }}\"\n");
    out.push_str("        state: latest\n");
    out.push_str("        enablerepo: elrepo\n\n");

    out.push_str("    - name: 6. Version-lock the installed kernel and DRBD modules\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str("        cmd: \"dnf versionlock add {{ item }}\"\n");
    out.push_str("      loop: \"{{ kernel_packages + drbd_packages }}\"\n");
    out.push_str("      register: lock_result\n");
    out.push_str("      changed_when: \"'adding' in lock_result.stdout\"\n\n");

    out.push_str("    - name: 7. Reboot only if the kernel was updated\n");
    out.push_str("      ansible.builtin.reboot:\n");
    out.push_str("        reboot_timeout: 1800\n");
    out.push_str("      when: kernel_update_result.changed\n\n");

    out.push_str("    - name: 8. Configure automatic DRBD kernel module loading\n");
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str("        content: \"drbd\"\n");
    out.push_str("        dest: /etc/modules-load.d/drbd.conf\n");
    out.push_str("        mode: '0644'\n\n");

    out.push_str("    - name: 9. Load the DRBD kernel module now\n");
    out.push_str("      community.general.modprobe:\n");
    out.push_str("        name: drbd\n");
    out.push_str("        state: present\n\n");

    out.push_str(&format!("    - name: 10. Open the firewall ports (DRBD {})\n", port_range));
    out.push_str("      ansible.posix.firewalld:\n");
    out.push_str("        zone: work\n");
    out.push_str(&format!("        port: \"{}\"\n", port_range));
    out.push_str("        permanent: yes\n");
    out.push_str("        state: enabled\n");
    out.push_str("      notify: Reload Firewalld\n\n");

    // fencing=resource-only 인 경우에만 fence-peer 핸들러 등록
    // (핸들러가 없으면 피어 단절 시 promote가 op timeout까지 hang 됨)
    let needs_fence_peer_handler = resource.disk_options.fencing == "resource-only";
    if needs_fence_peer_handler {
        out.push_str("    - name: 11. Deploy the global DRBD settings (global_common.conf)\n");
        out.push_str("      # The resource-only fencing policy needs a fence-peer handler.\n");
        out.push_str("      # Without it, a promote hangs until the op timeout after a peer disconnect.\n");
        out.push_str("      ansible.builtin.copy:\n");
        out.push_str("        dest: /etc/drbd.d/global_common.conf\n");
        out.push_str("        content: |\n");
        for line in generate_global_common_conf().lines() {
            out.push_str(&format!("          {}\n", line));
        }
        out.push_str("        mode: '0644'\n");
        out.push_str("        backup: yes\n");
        out.push_str("      notify: Adjust DRBD config\n\n");
    }

    out.push_str("  handlers:\n");
    out.push_str("    - name: Reload Firewalld\n");
    out.push_str("      ansible.builtin.systemd:\n");
    out.push_str("        name: firewalld\n");
    out.push_str("        state: restarted\n\n");

    if needs_fence_peer_handler {
        out.push_str("    - name: Adjust DRBD config\n");
        out.push_str("      # drbdadm adjust only applies the diff between the local config file\n");
        out.push_str("      # and the kernel state, so it is a no-op when nothing changed and can\n");
        out.push_str("      # be re-run safely on every deployment without a reboot.\n");
        out.push_str("      ansible.builtin.command: drbdadm adjust all\n");
        out.push_str("      changed_when: true\n\n");
    }

    // ── Play 2: LVM + .res 파일 배포 ──────────────────────────
    out.push_str(&format!("- name: Configure the DRBD LVM volumes and resource\n"));
    out.push_str("  hosts: all\n");
    out.push_str("  become: true\n");
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));

    // drbd_resources 변수 정의
    out.push_str("    drbd_resources:\n");
    if has_lvm {
        // 항목마다 host를 실어 둔다. 이 play는 hosts: all 이라 host 가드가
        // 없으면 모든 노드가 *다른 노드의* VG에 LV를 만들려다 실패한다
        // (노드별 VG 이름이 다를 때 실제로 배포가 깨진다).
        for node in &lvm_nodes {
            let lv_name = res_name;
            let size = if node.lvm_size.is_empty() { "10G" } else { &node.lvm_size };
            out.push_str(&format!(
                "      - {{ host: {}, name: {}, lv_name: {}, vg: {}, size: {}, port: {}, device: /dev/drbd{} }}\n",
                node.hostname, res_name, lv_name, node.lvm_vg, size, node.port, resource.minor
            ));
        }
    } else {
        out.push_str(&format!(
            "      - {{ name: {}, port: {}, device: /dev/drbd{} }}\n",
            res_name, resource.nodes.first().map(|n| n.port).unwrap_or(7789), resource.minor
        ));
    }

    out.push_str("  tasks:\n");

    if has_lvm {
        out.push_str("    - name: 1. Create the LVM logical volumes\n");
        out.push_str("      community.general.lvol:\n");
        out.push_str("        vg: \"{{ item.vg }}\"\n");
        out.push_str("        lv: \"{{ item.lv_name }}\"\n");
        out.push_str("        size: \"{{ item.size }}\"\n");
        out.push_str("      loop: \"{{ drbd_resources }}\"\n");
        out.push_str("      # Each node only creates an LV in its own VG --\n");
        out.push_str("      # without this guard, lvol fails on another node's VG name.\n");
        out.push_str("      when: item.host == inventory_hostname\n");
        out.push_str("      loop_control:\n");
        out.push_str("        label: \"{{ item.host }}:{{ item.vg }}/{{ item.lv_name }}\"\n\n");
    }

    out.push_str("    - name: Deploy the DRBD resource file (.res)\n");
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str(&format!("        dest: /etc/drbd.d/{}.res\n", res_name));
    out.push_str("        content: |\n");
    for line in generate_res_file(resource).lines() {
        out.push_str(&format!("          {}\n", line));
    }
    out.push_str("        mode: '0644'\n\n");

    out.push_str("    - name: Check whether the device is up (drbdadm status)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm status {}\"\n", res_name));
    out.push_str("      register: check_status\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    out.push_str("    - name: Check whether metadata exists (drbdadm dump-md)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm dump-md {}\"\n", res_name));
    out.push_str("      register: check_md\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    out.push_str("    - name: Create the DRBD metadata (only when missing)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm create-md {}\"\n", res_name));
    out.push_str("      when:\n");
    out.push_str("        - check_md.rc != 0\n");
    out.push_str("        - check_status.rc != 0\n\n");

    out.push_str("    - name: Bring the DRBD resource up\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm up {}\"\n", res_name));
    out.push_str("      register: up_result\n");
    out.push_str("      changed_when: \"'already' not in up_result.stderr and up_result.rc == 0\"\n");
    out.push_str("      failed_when: \"up_result.rc != 0 and 'already' not in up_result.stderr\"\n\n");

    out.push_str(&format!(
        "    - name: Force the initial Primary on the first node ({}) only\n", first_node
    ));
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm primary --force {}\"\n", res_name));
    out.push_str(&format!("      when: inventory_hostname == \"{}\"\n", first_node));
    out.push_str("      register: primary_result\n");
    out.push_str("      changed_when: primary_result.rc == 0\n");
    out.push_str("      failed_when:\n");
    out.push_str("        - primary_result.rc != 0\n");
    out.push_str("        - \"'already' not in primary_result.stderr\"\n\n");

    out.push_str("    - name: Check the synchronization state\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm status {}\"\n", res_name));
    out.push_str("      changed_when: false\n");

    // suppress unused warning
    let _ = inventory;

    out
}

// ─────────────────────────────────────────────────────────────
// DRBD 초기화 명령 생성 (참고용)
// ─────────────────────────────────────────────────────────────

pub fn generate_drbd_init_commands(resource: &DrbdResource) -> Vec<String> {
    let mut cmds = Vec::new();
    cmds.push("# Run on every node: initialize the DRBD metadata".to_string());
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] drbdadm create-md {}",
            node.hostname, resource.resource_name
        ));
    }
    cmds.push("# Run on every node: start the DRBD service".to_string());
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] systemctl enable --now drbd",
            node.hostname
        ));
    }
    cmds.push("# Force the initial sync from the first node".to_string());
    if let Some(first) = resource.nodes.first() {
        cmds.push(format!(
            "# [{}] drbdadm -- --overwrite-data-of-peer primary {}",
            first.hostname, resource.resource_name
        ));
    }
    cmds
}

// ─────────────────────────────────────────────────────────────
// /etc/drbd.d/*.res 파일 파서
// ─────────────────────────────────────────────────────────────

/// 단일 .res 파일 내용을 파싱하여 ScannedDrbdResource 반환
pub fn parse_res_file(content: &str, source_file: &str) -> Option<ScannedDrbdResource> {
    // resource <name> {
    let resource_name = content.lines()
        .find_map(|line| {
            let t = line.trim();
            if t.starts_with("resource ") && t.ends_with('{') {
                let name = t["resource ".len()..t.len() - 1].trim();
                Some(name.to_string())
            } else {
                None
            }
        })?;

    // protocol <X>;
    let protocol = content.lines()
        .find_map(|line| {
            let t = line.trim();
            if t.starts_with("protocol ") {
                Some(t["protocol ".len()..].trim_end_matches(';').trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "C".to_string());

    // minor: 첫 번째 device 줄에서 추출
    let minor = content.lines()
        .find_map(|line| {
            let t = line.trim();
            if t.starts_with("device") && t.contains("/dev/drbd") {
                let part = t.split("/dev/drbd").nth(1)?;
                part.trim_end_matches(';').trim().parse::<u32>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    // on <hostname> { ... } 블록 파싱 (단순 상태 머신)
    let nodes = parse_on_blocks(content);

    Some(ScannedDrbdResource {
        resource_name,
        protocol,
        minor,
        nodes,
        source_file: source_file.to_string(),
    })
}

fn parse_on_blocks(content: &str) -> Vec<ScannedDrbdNode> {
    let mut nodes = Vec::new();
    let mut depth: i32 = 0;            // 중첩 깊이
    let mut in_on = false;             // on <host> { 블록 안
    let mut current_hostname = String::new();
    let mut block_lines: Vec<String> = Vec::new();
    let mut resource_depth: i32 = 0;  // resource { 의 깊이

    for line in content.lines() {
        let t = line.trim();

        // 깊이 계산
        let opens  = t.chars().filter(|&c| c == '{').count() as i32;
        let closes = t.chars().filter(|&c| c == '}').count() as i32;

        // resource { 검출 (depth == 0)
        if depth == 0 && t.starts_with("resource ") && t.ends_with('{') {
            depth += opens - closes;
            resource_depth = depth;
            continue;
        }

        if depth > 0 {
            // on <hostname> { 검출 (resource 바로 아래, depth == resource_depth)
            if !in_on && depth == resource_depth && t.starts_with("on ") && t.ends_with('{') {
                let hostname = t["on ".len()..t.len() - 1].trim().to_string();
                current_hostname = hostname;
                block_lines.clear();
                in_on = true;
                depth += opens - closes;
                continue;
            }

            // in_on 블록 내용 수집
            if in_on {
                // on 블록 닫힘 감지
                if closes > 0 && (depth + opens - closes) == resource_depth {
                    // 블록 끝
                    depth += opens - closes;
                    let node = build_scanned_node(&current_hostname, &block_lines);
                    nodes.push(node);
                    in_on = false;
                    block_lines.clear();
                    current_hostname.clear();
                    continue;
                }
                block_lines.push(line.to_string());
            }

            depth += opens - closes;
        } else if depth < 0 {
            depth = 0;
        }
    }

    nodes
}

fn build_scanned_node(hostname: &str, lines: &[String]) -> ScannedDrbdNode {
    let disk    = extract_kv(lines, "disk");
    let address = extract_kv(lines, "address").unwrap_or_default();
    let meta    = extract_kv(lines, "meta-disk").unwrap_or_else(|| "internal".to_string());

    // address 는 "[<family>] <ip>:<port>" 형태이고 IPv6 는 [addr]:port 다.
    // 파싱은 scan.rs 와 같은 규칙을 쓴다 (family 키워드/대괄호 처리).
    let ip = crate::scan::parse_drbd_address(&format!("address {}", address))
        .unwrap_or_else(|| address.clone());
    let port = address
        .trim_end_matches(';')
        .rsplit_once(':')
        .and_then(|(_, p)| p.trim().parse::<u16>().ok())
        .unwrap_or(7789);

    // LVM 자동 감지: /dev/<vg>/<lv> 형태 (path component 3개)
    let disk_str = disk.clone().unwrap_or_default();
    let parts: Vec<&str> = disk_str.trim_start_matches('/').split('/').collect();
    // /dev/<vg>/<lv> → ["dev", "<vg>", "<lv>"]
    let (disk_type, lvm_vg) = if parts.len() == 3 && parts[0] == "dev" {
        // 일반 블록 장치 이름 패턴 제외: sda/sdb/nvme.../vd.../xvd...
        let second = parts[1];
        let is_raw = second.starts_with("sd") || second.starts_with("hd")
            || second.starts_with("vd") || second.starts_with("xvd")
            || second.starts_with("nvme") || second.starts_with("drbd");
        if is_raw {
            ("block".to_string(), String::new())
        } else {
            ("lvm".to_string(), second.to_string())
        }
    } else {
        ("block".to_string(), String::new())
    };

    ScannedDrbdNode {
        hostname: hostname.to_string(),
        ip,
        port,
        disk: disk_str.clone(), // LVM도 원본 경로 보존 (JS lvscan 매칭용; 생성 시 effective_disk()가 재계산)
        meta,
        disk_type,
        lvm_vg,
        lvm_size: String::new(), // .res 파일에는 크기 정보 없음
    }
}

fn extract_kv(lines: &[String], key: &str) -> Option<String> {
    for line in lines {
        let t = line.trim();
        if t.starts_with(key) {
            let rest = t[key.len()..].trim().trim_end_matches(';').trim();
            return Some(rest.to_string());
        }
    }
    None
}

/// /etc/drbd.d 디렉토리의 모든 .res 파일을 스캔
pub fn scan_drbd_dir(dir: &str) -> Vec<ScannedDrbdResource> {
    let mut resources = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return resources,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("res") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let filename = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(res) = parse_res_file(&content, &filename) {
            resources.push(res);
        }
    }

    resources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::drbd::DrbdNode;

    fn sample_resource(fencing: &str) -> DrbdResource {
        let mut resource = DrbdResource {
            resource_name: "r0".to_string(),
            nodes: vec![
                DrbdNode {
                    hostname: "node1".to_string(),
                    ip: "192.168.10.11".to_string(),
                    port: 7789,
                    ..DrbdNode::default()
                },
                DrbdNode {
                    hostname: "node2".to_string(),
                    ip: "192.168.10.12".to_string(),
                    port: 7789,
                    ..DrbdNode::default()
                },
            ],
            ..DrbdResource::default()
        };
        resource.disk_options.fencing = fencing.to_string();
        resource
    }

    #[test]
    fn global_common_conf_contains_fence_peer_handlers() {
        let conf = generate_global_common_conf();
        assert!(conf.contains("fence-peer \"/usr/lib/drbd/crm-fence-peer.9.sh\";"));
        assert!(conf.contains("after-resync-target \"/usr/lib/drbd/crm-unfence-peer.9.sh\";"));
        assert!(conf.contains("usage-count yes;"));
    }

    #[test]
    fn ansible_playbook_deploys_global_common_conf_when_fencing_is_resource_only() {
        let resource = sample_resource("resource-only");
        let inventory = AnsibleInventory::default();
        let playbook = generate_ansible_playbook(&resource, &inventory);

        assert!(playbook.contains("global_common.conf"));
        assert!(playbook.contains("fence-peer"));
        assert!(playbook.contains("notify: Adjust DRBD config"));
        assert!(playbook.contains("drbdadm adjust all"));
    }

    /// 폼의 wfc-timeout / degr-wfc-timeout 값이 .res에 실제로 나타나야
    /// 한다. 이전에는 값을 받아 DB에 저장만 하고 생성물에는 전혀 쓰지
    /// 않아 사용자가 무엇을 입력하든 결과가 같았다.
    #[test]
    fn res_file_emits_startup_block_with_configured_timeouts() {
        let mut resource = sample_resource("resource-only");
        resource.startup_options.wfc_timeout = 42;
        resource.startup_options.degr_wfc_timeout = 99;

        let res = generate_res_file(&resource);
        assert!(res.contains("startup {"));
        assert!(res.contains("wfc-timeout      42;"));
        assert!(res.contains("degr-wfc-timeout 99;"));
    }

    /// `become-primary-on`은 DRBD 9 drbdadm이 조용히 버리는 키워드이고,
    /// Primary 승격은 Pacemaker promotable clone이 결정한다. 생성물에
    /// 들어가면 "설정했는데 동작하지 않는" 오해를 만든다.
    #[test]
    fn res_file_never_emits_become_primary_on() {
        let mut resource = sample_resource("resource-only");
        resource.startup_options.become_primary_on = "node1".to_string();
        assert!(!generate_res_file(&resource).contains("become-primary-on"));
    }

    #[test]
    fn on_io_error_only_emits_values_drbd9_accepts() {
        // drbdadm 9.34 허용값: pass_on | call-local-io-error | detach
        for (input, expected) in [
            ("detach", "detach"),
            ("pass_on", "pass_on"),
            ("call-local-io-error", "call-local-io-error"),
            // 레거시(무효) 값 → 유효값으로 이전
            ("passthrough", "pass_on"),
            ("panic", "call-local-io-error"),
            ("", "detach"),
        ] {
            let mut r = sample_resource("resource-only");
            r.disk_options.on_io_error = input.to_string();
            let res = generate_res_file(&r);
            assert!(
                res.contains(&format!("on-io-error {};", expected)),
                "on-io-error {:?} → {:?} 기대, 생성물:\n{}",
                input,
                expected,
                res
            );
        }
    }

    #[test]
    fn after_sb_1pri_only_emits_values_drbd9_accepts() {
        // drbdadm 9.34 허용값: disconnect | consensus | discard-secondary
        //                     | call-pri-lost-after-sb | violently-as0p
        for (input, expected) in [
            ("discard-secondary", "discard-secondary"),
            ("consensus", "consensus"),
            ("violently-as0p", "violently-as0p"),
            // `call-pri-lost`는 유효한 이름이 아니다
            ("call-pri-lost", "call-pri-lost-after-sb"),
        ] {
            let mut r = sample_resource("resource-only");
            r.net_options.after_sb_1pri = input.to_string();
            let res = generate_res_file(&r);
            assert!(
                res.contains(&format!("after-sb-1pri {};", expected)),
                "after-sb-1pri {:?} → {:?} 기대",
                input,
                expected
            );
        }
    }

    /// 기본 설정 그대로 만든 .res가 DRBD 9 문법을 지켜야 한다.
    /// (예전 기본값 `on-io-error passthrough`는 drbdadm이 거부했다.)
    #[test]
    fn default_res_file_uses_no_drbd8_only_keywords() {
        let res = generate_res_file(&sample_resource("resource-only"));
        for invalid in ["passthrough", "panic", "call-pri-lost;", "become-primary-on"] {
            assert!(
                !res.contains(invalid),
                "DRBD 9가 거부하는 값 {:?}이 생성물에 있다:\n{}",
                invalid,
                res
            );
        }
    }

    fn lvm_resource(vgs: &[(&str, &str)]) -> DrbdResource {
        DrbdResource {
            resource_name: "r0".to_string(),
            minor: 0,
            nodes: vgs
                .iter()
                .map(|(host, vg)| DrbdNode {
                    hostname: host.to_string(),
                    ip: "192.168.10.11".to_string(),
                    port: 7789,
                    disk_type: "lvm".to_string(),
                    lvm_vg: vg.to_string(),
                    lvm_size: "10G".to_string(),
                    ..DrbdNode::default()
                })
                .collect(),
            ..DrbdResource::default()
        }
    }

    /// 이 play는 `hosts: all`이라 host 가드가 없으면 모든 노드가 *다른
    /// 노드의* VG에 LV를 만들려다 실패한다 (노드별 VG 이름이 다를 때
    /// 실제로 배포가 깨졌다).
    #[test]
    fn lvm_playbook_guards_lv_creation_to_the_owning_host() {
        let resource = lvm_resource(&[("node1", "vg_a"), ("node2", "vg_b")]);
        let playbook = generate_ansible_playbook(&resource, &AnsibleInventory::default());

        assert!(playbook.contains("when: item.host == inventory_hostname"));
        assert!(playbook.contains("{ host: node1, name: r0, lv_name: r0, vg: vg_a,"));
        assert!(playbook.contains("{ host: node2, name: r0, lv_name: r0, vg: vg_b,"));
        // 각 VG는 자기 노드 항목에만 등장해야 한다.
        assert_eq!(playbook.matches("vg: vg_a").count(), 1);
        assert_eq!(playbook.matches("vg: vg_b").count(), 1);
    }

    /// `loop:`가 참조하는 `drbd_resources`는 어떤 입력에서도 비어 있지
    /// 않은 리스트여야 한다. VG 이름이 없는 노드를 걸러내지 않으면 이
    /// 키가 null이 되어 "Invalid data passed to loop"로 죽는다.
    fn drbd_resources_var(playbook: &str) -> Vec<serde_yaml::Value> {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(playbook).expect("플레이북 YAML 파싱 실패");
        let seq = doc
            .as_sequence()
            .expect("플레이북은 play 리스트여야 한다")
            .iter()
            .filter_map(|play| play.get("vars"))
            .find_map(|vars| vars.get("drbd_resources"))
            .expect("drbd_resources 변수가 없다")
            .as_sequence()
            .expect("drbd_resources는 null이 아닌 리스트여야 한다 (loop가 깨진다)")
            .clone();
        assert!(!seq.is_empty(), "drbd_resources가 비어 있다");
        seq
    }

    #[test]
    fn lvm_entries_without_vg_do_not_leave_an_empty_loop_variable() {
        let resource = lvm_resource(&[("node1", ""), ("node2", "")]);
        let playbook = generate_ansible_playbook(&resource, &AnsibleInventory::default());

        // VG가 없으면 lvol 태스크 자체를 만들지 않는다.
        assert!(!playbook.contains("community.general.lvol"));
        let entries = drbd_resources_var(&playbook);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn lvm_loop_variable_has_one_entry_per_owning_node() {
        let resource = lvm_resource(&[("node1", "vg_a"), ("node2", "vg_b")]);
        let playbook = generate_ansible_playbook(&resource, &AnsibleInventory::default());

        let entries = drbd_resources_var(&playbook);
        assert_eq!(entries.len(), 2);
        let pairs: Vec<(String, String)> = entries
            .iter()
            .map(|e| {
                (
                    e["host"].as_str().expect("host 키 필요").to_string(),
                    e["vg"].as_str().expect("vg 키 필요").to_string(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("node1".to_string(), "vg_a".to_string()),
                ("node2".to_string(), "vg_b".to_string()),
            ]
        );
    }

    /// 생성물이 Ansible에 먹히려면 우선 YAML로 파싱돼야 한다.
    #[test]
    fn generated_playbook_is_valid_yaml() {
        for resource in [
            lvm_resource(&[("node1", "vg_a"), ("node2", "vg_b")]),
            sample_resource("resource-only"),
            sample_resource("dont-care"),
        ] {
            let playbook = generate_ansible_playbook(&resource, &AnsibleInventory::default());
            let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&playbook);
            assert!(
                parsed.is_ok(),
                "YAML 파싱 실패: {:?}\n---\n{}",
                parsed.err(),
                playbook
            );
        }
    }

    #[test]
    fn ansible_playbook_skips_global_common_conf_when_fencing_is_not_resource_only() {
        let resource = sample_resource("dont-care");
        let inventory = AnsibleInventory::default();
        let playbook = generate_ansible_playbook(&resource, &inventory);

        assert!(!playbook.contains("global_common.conf"));
        assert!(!playbook.contains("Adjust DRBD config"));
    }


}

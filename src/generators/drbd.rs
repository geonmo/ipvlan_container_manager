use crate::models::drbd::{AnsibleInventory, DrbdResource, ScannedDrbdNode, ScannedDrbdResource};
use anyhow::Result;

// ─────────────────────────────────────────────────────────────
// .res 파일 생성
// ─────────────────────────────────────────────────────────────

pub fn generate_res_file(resource: &DrbdResource) -> String {
    let mut out = String::new();

    out.push_str(&format!("resource {} {{\n", resource.resource_name));
    out.push_str(&format!("    protocol {};\n\n", resource.protocol));

    // options 블록
    out.push_str("    options {\n");
    out.push_str("        quorum majority;\n");
    out.push_str("        on-no-quorum io-error;\n");
    out.push_str("    }\n\n");

    // disk 블록 (on-io-error만, fencing은 net으로)
    out.push_str("    disk {\n");
    out.push_str(&format!(
        "        on-io-error {};\n",
        resource.disk_options.on_io_error
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
        resource.net_options.after_sb_1pri
    ));
    if resource.net_options.after_sb_2pri != "disconnect" || resource.net_options.allow_two_primaries {
        out.push_str(&format!(
            "        after-sb-2pri {};\n",
            resource.net_options.after_sb_2pri
        ));
    }
    out.push_str("    }\n\n");

    // on <node> sections (node-id 포함)
    out.push_str("    # 인벤토리 순서대로 node-id를 0, 1, 2... 순으로 부여합니다.\n");
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
    let has_lvm = resource.nodes.iter().any(|n| n.disk_type == "lvm");
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
    out.push_str(&format!("# DRBD 리소스 '{}' 배포 플레이북 (자동 생성)\n\n", res_name));

    // ── Play 1: DRBD 패키지 설치 (hosts: drbd) ─────────────────
    out.push_str(&format!("- name: DRBD 패키지 설치\n"));
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

    out.push_str("    - name: 1. ELRepo GPG 키 가져오기\n");
    out.push_str("      ansible.builtin.rpm_key:\n");
    out.push_str("        state: present\n");
    out.push_str("        key: https://www.elrepo.org/RPM-GPG-KEY-elrepo.org\n\n");

    out.push_str("    - name: 2. ELRepo 저장소 설치 (AlmaLinux 9용)\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: https://www.elrepo.org/elrepo-release-9.el9.elrepo.noarch.rpm\n");
    out.push_str("        state: present\n");
    out.push_str("        disable_gpg_check: yes\n\n");

    out.push_str("    - name: 3. dnf-plugin-versionlock 설치\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: python3-dnf-plugin-versionlock\n");
    out.push_str("        state: present\n\n");

    out.push_str("    - name: 4. 커널 패키지 업데이트\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: \"{{ kernel_packages }}\"\n");
    out.push_str("        state: latest\n");
    out.push_str("        enablerepo: baseos,appstream\n");
    out.push_str("      register: kernel_update_result\n\n");

    out.push_str("    - name: 5. DRBD 관련 패키지 설치\n");
    out.push_str("      ansible.builtin.dnf:\n");
    out.push_str("        name: \"{{ drbd_packages }}\"\n");
    out.push_str("        state: latest\n");
    out.push_str("        enablerepo: elrepo\n\n");

    out.push_str("    - name: 6. 설치된 커널 및 DRBD 모듈 버전 고정\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str("        cmd: \"dnf versionlock add {{ item }}\"\n");
    out.push_str("      loop: \"{{ kernel_packages + drbd_packages }}\"\n");
    out.push_str("      register: lock_result\n");
    out.push_str("      changed_when: \"'adding' in lock_result.stdout\"\n\n");

    out.push_str("    - name: 7. 커널 업데이트 시에만 재부팅\n");
    out.push_str("      ansible.builtin.reboot:\n");
    out.push_str("        reboot_timeout: 1800\n");
    out.push_str("      when: kernel_update_result.changed\n\n");

    out.push_str("    - name: 8. DRBD 커널 모듈 자동 로드 설정\n");
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str("        content: \"drbd\"\n");
    out.push_str("        dest: /etc/modules-load.d/drbd.conf\n");
    out.push_str("        mode: '0644'\n\n");

    out.push_str("    - name: 9. DRBD 커널 모듈 즉시 로드\n");
    out.push_str("      community.general.modprobe:\n");
    out.push_str("        name: drbd\n");
    out.push_str("        state: present\n\n");

    out.push_str(&format!("    - name: 10. 방화벽 포트 허용 (DRBD {})\n", port_range));
    out.push_str("      ansible.posix.firewalld:\n");
    out.push_str("        zone: work\n");
    out.push_str(&format!("        port: \"{}\"\n", port_range));
    out.push_str("        permanent: yes\n");
    out.push_str("        state: enabled\n");
    out.push_str("      notify: Reload Firewalld\n\n");

    out.push_str("  handlers:\n");
    out.push_str("    - name: Reload Firewalld\n");
    out.push_str("      ansible.builtin.systemd:\n");
    out.push_str("        name: firewalld\n");
    out.push_str("        state: restarted\n\n");

    // ── Play 2: LVM + .res 파일 배포 ──────────────────────────
    out.push_str(&format!("- name: DRBD LVM 볼륨 및 리소스 설정\n"));
    out.push_str("  hosts: all\n");
    out.push_str("  become: true\n");
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));

    // drbd_resources 변수 정의
    out.push_str("    drbd_resources:\n");
    if has_lvm {
        for node in &resource.nodes {
            if node.disk_type == "lvm" && !node.lvm_vg.is_empty() {
                let lv_name = res_name;
                let size = if node.lvm_size.is_empty() { "10G" } else { &node.lvm_size };
                out.push_str(&format!(
                    "      # host: {}\n      - {{ name: {}, lv_name: {}, vg: {}, size: {}, port: {}, device: /dev/drbd{} }}\n",
                    node.hostname, res_name, lv_name, node.lvm_vg, size, node.port, resource.minor
                ));
            }
        }
    } else {
        out.push_str(&format!(
            "      - {{ name: {}, port: {}, device: /dev/drbd{} }}\n",
            res_name, resource.nodes.first().map(|n| n.port).unwrap_or(7789), resource.minor
        ));
    }

    out.push_str("  tasks:\n");

    if has_lvm {
        out.push_str("    - name: 1. LVM 논리 볼륨(LV) 생성\n");
        out.push_str("      community.general.lvol:\n");
        out.push_str("        vg: \"{{ item.vg }}\"\n");
        out.push_str("        lv: \"{{ item.lv_name }}\"\n");
        out.push_str("        size: \"{{ item.size }}\"\n");
        out.push_str("      loop: \"{{ drbd_resources }}\"\n\n");
    }

    out.push_str("    - name: DRBD 리소스 설정 파일(.res) 배포\n");
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str(&format!("        dest: /etc/drbd.d/{}.res\n", res_name));
    out.push_str("        content: |\n");
    for line in generate_res_file(resource).lines() {
        out.push_str(&format!("          {}\n", line));
    }
    out.push_str("        mode: '0644'\n\n");

    out.push_str("    - name: 장치 활성화 상태 확인 (drbdadm status)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm status {}\"\n", res_name));
    out.push_str("      register: check_status\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    out.push_str("    - name: 메타데이터 존재 여부 확인 (drbdadm dump-md)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm dump-md {}\"\n", res_name));
    out.push_str("      register: check_md\n");
    out.push_str("      failed_when: false\n");
    out.push_str("      changed_when: false\n\n");

    out.push_str("    - name: DRBD 메타데이터 생성 (미존재 시에만 실행)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm create-md {}\"\n", res_name));
    out.push_str("      when:\n");
    out.push_str("        - check_md.rc != 0\n");
    out.push_str("        - check_status.rc != 0\n\n");

    out.push_str("    - name: DRBD 리소스 Up (활성화)\n");
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm up {}\"\n", res_name));
    out.push_str("      register: up_result\n");
    out.push_str("      changed_when: \"'already' not in up_result.stderr and up_result.rc == 0\"\n");
    out.push_str("      failed_when: \"up_result.rc != 0 and 'already' not in up_result.stderr\"\n\n");

    out.push_str(&format!(
        "    - name: 첫 번째 노드({})에서만 초기 Primary 강제 지정\n", first_node
    ));
    out.push_str("      ansible.builtin.command:\n");
    out.push_str(&format!("        cmd: \"drbdadm primary --force {}\"\n", res_name));
    out.push_str(&format!("      when: inventory_hostname == \"{}\"\n", first_node));
    out.push_str("      register: primary_result\n");
    out.push_str("      changed_when: primary_result.rc == 0\n");
    out.push_str("      failed_when:\n");
    out.push_str("        - primary_result.rc != 0\n");
    out.push_str("        - \"'already' not in primary_result.stderr\"\n\n");

    out.push_str("    - name: 동기화 상태 확인\n");
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
    cmds.push("# 모든 노드에서 실행: DRBD 메타데이터 초기화".to_string());
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] drbdadm create-md {}",
            node.hostname, resource.resource_name
        ));
    }
    cmds.push("# 모든 노드에서 실행: DRBD 서비스 시작".to_string());
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] systemctl enable --now drbd",
            node.hostname
        ));
    }
    cmds.push("# 첫 번째 노드에서 초기 동기화 강제 시작".to_string());
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

    // address = "ip:port"
    let (ip, port) = if let Some(pos) = address.rfind(':') {
        let ip   = address[..pos].to_string();
        let port = address[pos + 1..].parse::<u16>().unwrap_or(7789);
        (ip, port)
    } else {
        (address, 7789)
    };

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

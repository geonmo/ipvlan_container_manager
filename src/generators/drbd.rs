use crate::models::drbd::{DrbdResource, AnsibleInventory};
use anyhow::Result;

/// DRBD .res 파일 생성
pub fn generate_res_file(resource: &DrbdResource) -> String {
    let mut out = String::new();

    out.push_str(&format!("resource {} {{\n\n", resource.resource_name));

    // protocol
    out.push_str(&format!("    protocol {};\n\n", resource.protocol));

    // startup
    out.push_str("    startup {\n");
    if !resource.startup_options.become_primary_on.is_empty() {
        out.push_str(&format!(
            "        become-primary-on {};\n",
            resource.startup_options.become_primary_on
        ));
    }
    out.push_str(&format!(
        "        wfc-timeout {};\n",
        resource.startup_options.wfc_timeout
    ));
    out.push_str(&format!(
        "        degr-wfc-timeout {};\n",
        resource.startup_options.degr_wfc_timeout
    ));
    out.push_str("    }\n\n");

    // net
    out.push_str("    net {\n");
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
    out.push_str(&format!(
        "        after-sb-2pri {};\n",
        resource.net_options.after_sb_2pri
    ));
    out.push_str("    }\n\n");

    // disk
    out.push_str("    disk {\n");
    out.push_str(&format!(
        "        on-io-error {};\n",
        resource.disk_options.on_io_error
    ));
    out.push_str(&format!(
        "        fencing {};\n",
        resource.disk_options.fencing
    ));
    out.push_str("    }\n\n");

    // on <node> sections
    for node in &resource.nodes {
        out.push_str(&format!("    on {} {{\n", node.hostname));
        out.push_str(&format!(
            "        device      /dev/drbd{};\n",
            resource.minor
        ));
        out.push_str(&format!("        disk        {};\n", node.disk_device));
        out.push_str(&format!(
            "        address     {}:{};\n",
            node.ip, node.port
        ));
        out.push_str(&format!("        meta-disk   {};\n", node.meta_disk));
        out.push_str("    }\n\n");
    }

    out.push_str("}\n");
    out
}

/// Ansible 인벤토리(YAML) 생성
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

/// DRBD 초기화 bash 커맨드 생성 (참고용 출력)
pub fn generate_drbd_init_commands(resource: &DrbdResource) -> Vec<String> {
    let mut cmds = Vec::new();
    cmds.push(format!(
        "# 양쪽 노드에서 실행: DRBD 메타데이터 초기화"
    ));
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] drbdadm create-md {}",
            node.hostname, resource.resource_name
        ));
    }
    cmds.push(format!(
        "# 양쪽 노드에서 실행: DRBD 서비스 시작"
    ));
    for node in &resource.nodes {
        cmds.push(format!(
            "# [{}] systemctl enable --now drbd",
            node.hostname
        ));
    }
    cmds.push(format!(
        "# 첫 번째 노드에서 초기 동기화 강제 시작"
    ));
    if let Some(first) = resource.nodes.first() {
        cmds.push(format!(
            "# [{}] drbdadm -- --overwrite-data-of-peer primary {}",
            first.hostname, resource.resource_name
        ));
    }
    cmds
}

use crate::models::pacemaker::{
    ColocationConstraint, DrbdPacemakerResource, FsResource, LocationConstraint,
    OrderConstraint, PacemakerConfig, ResourceGroup, SystemdResource,
};

/// pcs 명령 스크립트 전체 생성
pub fn generate_pcs_script(config: &PacemakerConfig) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("#!/bin/bash".to_string());
    lines.push("# Pacemaker 설정 스크립트 (pcs 명령)".to_string());
    lines.push("# AlmaLinux9 / RHEL9 + pacemaker + corosync".to_string());
    lines.push("# 반드시 클러스터 노드 중 하나에서 root 권한으로 실행하세요.".to_string());
    lines.push(String::new());
    lines.push("set -euo pipefail".to_string());
    lines.push(String::new());

    // 클러스터 전역 설정
    lines.push("# ─── 클러스터 전역 속성 ───────────────────────────────────────".to_string());
    lines.push(format!(
        "pcs property set stonith-enabled={}",
        config.cluster.stonith_enabled
    ));
    lines.push(format!(
        "pcs property set no-quorum-policy={}",
        config.cluster.no_quorum_policy
    ));
    lines.push(format!(
        "pcs resource defaults migration-threshold={}",
        config.cluster.migration_threshold
    ));
    lines.push(format!(
        "pcs resource defaults failure-timeout={}",
        config.cluster.failure_timeout
    ));
    lines.push(String::new());

    // DRBD Promotable Clone 리소스
    if !config.drbd_resources.is_empty() {
        lines.push("# ─── DRBD Promotable Clone 리소스 ───────────────────────────".to_string());
        for drbd in &config.drbd_resources {
            lines.extend(generate_drbd_resource_cmds(drbd));
        }
        lines.push(String::new());
    }

    // Filesystem 리소스 (DRBD 볼륨 위 마운트)
    if !config.fs_resources.is_empty() {
        lines.push("# ─── Filesystem 리소스 (ocf:heartbeat:Filesystem) ───────────".to_string());
        lines.push("# DRBD가 Primary로 승격된 후 볼륨을 마운트합니다.".to_string());
        for fs in &config.fs_resources {
            lines.extend(generate_fs_resource_cmds(fs));
        }
        lines.push(String::new());
    }

    // Systemd (Quadlet) 리소스
    if !config.systemd_resources.is_empty() {
        lines.push("# ─── Quadlet Systemd 리소스 (Pod / Container) ───────────────".to_string());
        for svc in &config.systemd_resources {
            lines.extend(generate_systemd_resource_cmds(svc));
        }
        lines.push(String::new());
    }

    // 리소스 그룹 (Pod + Container들을 순서대로 묶음)
    if !config.resource_groups.is_empty() {
        lines.push("# ─── 리소스 그룹 (Pod → Container 순서 보장) ───────────────".to_string());
        lines.push("# 그룹 내 start: members 순서대로, stop: 역순 자동 처리".to_string());
        for grp in &config.resource_groups {
            lines.push(generate_resource_group_cmd(grp));
        }
        lines.push(String::new());
    }

    // Order 제약조건
    if !config.order_constraints.is_empty() {
        lines.push("# ─── Order 제약조건 ──────────────────────────────────────────".to_string());
        for ord in &config.order_constraints {
            lines.push(generate_order_constraint(ord));
        }
        lines.push(String::new());
    }

    // Colocation 제약조건
    if !config.colocation_constraints.is_empty() {
        lines.push("# ─── Colocation 제약조건 ────────────────────────────────────".to_string());
        for col in &config.colocation_constraints {
            lines.push(generate_colocation_constraint(col));
        }
        lines.push(String::new());
    }

    // Location 제약조건 (선호 노드)
    if !config.location_constraints.is_empty() {
        lines.push("# ─── Location 제약조건 (선호 노드) ─────────────────────────".to_string());
        for loc in &config.location_constraints {
            lines.push(generate_location_constraint(loc));
        }
        lines.push(String::new());
    }

    lines.push("echo '✅ Pacemaker 설정 완료'".to_string());
    lines.join("\n")
}

fn generate_drbd_resource_cmds(drbd: &DrbdPacemakerResource) -> Vec<String> {
    let mut cmds = Vec::new();

    cmds.push(format!(
        "pcs resource create {name} ocf:linbit:drbd \\",
        name = drbd.resource_name
    ));
    cmds.push(format!(
        "    drbd_resource={res} \\",
        res = drbd.drbd_resource_name
    ));
    cmds.push("    op monitor interval=29s role=Promoted \\".to_string());
    cmds.push("    op monitor interval=31s role=Unpromoted".to_string());

    cmds.push(format!(
        "pcs resource promotable {name} \\",
        name = drbd.resource_name
    ));
    cmds.push("    promoted-max=1 promoted-node-max=1 \\".to_string());
    cmds.push("    clone-max=2 clone-node-max=1 \\".to_string());
    cmds.push(format!(
        "    notify={notify} \\",
        notify = drbd.notify
    ));
    cmds.push(format!(
        "    id={clone_name}",
        clone_name = drbd.clone_name
    ));

    if let Some(node) = &drbd.target_role_master_node {
        cmds.push(format!(
            "pcs constraint location {clone} prefers {node}=200",
            clone = drbd.clone_name,
            node = node
        ));
    }

    cmds
}

fn generate_fs_resource_cmds(fs: &FsResource) -> Vec<String> {
    vec![
        format!(
            "pcs resource create {name} ocf:heartbeat:Filesystem \\",
            name = fs.resource_name
        ),
        format!("    device={dev} \\", dev = fs.device),
        format!("    directory={dir} \\", dir = fs.directory),
        format!("    fstype={fs} \\", fs = fs.fstype),
        format!("    op monitor interval={mon} \\", mon = fs.monitor_interval),
        format!("    op start  timeout={start} \\", start = fs.start_timeout),
        format!("    op stop   timeout={stop}", stop = fs.stop_timeout),
    ]
}

fn generate_systemd_resource_cmds(svc: &SystemdResource) -> Vec<String> {
    let mut cmds = Vec::new();

    cmds.push(format!(
        "pcs resource create {name} systemd:{unit} \\",
        name = svc.resource_name,
        unit = svc.systemd_unit,
    ));
    cmds.push(format!(
        "    op monitor interval={interval} \\",
        interval = svc.monitor_interval,
    ));
    cmds.push(format!(
        "    op start timeout={start} \\",
        start = svc.start_timeout,
    ));
    cmds.push(format!(
        "    op stop timeout={stop}",
        stop = svc.stop_timeout,
    ));

    if svc.clone {
        if let Some(clone_name) = &svc.clone_name {
            cmds.push(format!(
                "pcs resource clone {name} id={clone}",
                name = svc.resource_name,
                clone = clone_name,
            ));
        }
    }

    cmds
}

fn generate_resource_group_cmd(grp: &ResourceGroup) -> String {
    format!(
        "pcs resource group add {group} {members}",
        group = grp.group_name,
        members = grp.members.join(" "),
    )
}

fn generate_order_constraint(ord: &OrderConstraint) -> String {
    format!(
        "pcs constraint order {first_action} {first} then {then_action} {then} kind={kind} id={id}",
        first_action = ord.first_action,
        first = ord.first,
        then_action = ord.then_action,
        then = ord.then,
        kind = ord.kind,
        id = ord.id,
    )
}

fn generate_colocation_constraint(col: &ColocationConstraint) -> String {
    let rsc_role = col
        .rsc_role
        .as_deref()
        .map(|r| format!(" role={}", r))
        .unwrap_or_default();
    let with_role = col
        .with_rsc_role
        .as_deref()
        .map(|r| format!(" role={}", r))
        .unwrap_or_default();

    format!(
        "pcs constraint colocation add {rsc}{rsc_role} with {with}{with_role} score={score} id={id}",
        rsc = col.rsc,
        rsc_role = rsc_role,
        with = col.with_rsc,
        with_role = with_role,
        score = col.score,
        id = col.id,
    )
}

fn generate_location_constraint(loc: &LocationConstraint) -> String {
    format!(
        "pcs constraint location {rsc} prefers {node}={score} id={id}",
        rsc = loc.rsc,
        node = loc.node,
        score = loc.score,
        id = loc.id,
    )
}

/// 기본 권장 제약조건 자동 생성
///
/// 체인: DRBD promote → FS mount start → Group(Pod→Container) start
/// Move 시 역순: Group stop → FS unmount → DRBD demote
///
/// 케이스별 처리:
/// 1. DRBD + FS + Group  → DRBD→FS, FS→Group (권장)
/// 2. DRBD + FS (그룹 없음) → DRBD→FS, FS→각 systemd 리소스
/// 3. DRBD + 그룹 (FS 없음) → DRBD→Group
/// 4. DRBD만 (레거시) → DRBD→각 systemd 리소스
pub fn generate_default_constraints(config: &mut PacemakerConfig) {
    // 사전에 필요한 데이터를 복제 (borrow checker 회피)
    let drbd_list: Vec<_> = config.drbd_resources.clone();
    let fs_list: Vec<_> = config.fs_resources.clone();
    let svc_list: Vec<_> = config.systemd_resources.clone();
    let grp_list: Vec<_> = config.resource_groups.clone();

    for drbd in &drbd_list {
        if !fs_list.is_empty() {
            // ── 1단계: DRBD → FS ─────────────────────────────────────────────
            for fs in &fs_list {
                let linked_drbd = if fs.drbd_clone_name.is_empty() {
                    &drbd.clone_name
                } else {
                    &fs.drbd_clone_name
                };
                // FS가 다른 DRBD에 귀속된 경우 건너뜀
                if !fs.drbd_clone_name.is_empty() && fs.drbd_clone_name != drbd.clone_name {
                    continue;
                }

                config.order_constraints.push(OrderConstraint {
                    id: format!("ord-{}-after-{}", fs.resource_name, linked_drbd),
                    first: linked_drbd.clone(),
                    first_action: "promote".to_string(),
                    then: fs.resource_name.clone(),
                    then_action: "start".to_string(),
                    kind: "Mandatory".to_string(),
                });
                config.colocation_constraints.push(ColocationConstraint {
                    id: format!("col-{}-with-{}", fs.resource_name, linked_drbd),
                    rsc: fs.resource_name.clone(),
                    rsc_role: None,
                    with_rsc: linked_drbd.clone(),
                    with_rsc_role: Some("Promoted".to_string()),
                    score: "INFINITY".to_string(),
                });

                // ── 2단계: FS → Group 또는 FS → 개별 리소스 ─────────────────
                let groups_for_fs: Vec<&ResourceGroup> = grp_list
                    .iter()
                    .filter(|g| {
                        g.after_fs.as_deref() == Some(&fs.resource_name)
                            || g.after_fs.is_none()
                    })
                    .collect();

                if !groups_for_fs.is_empty() {
                    for grp in groups_for_fs {
                        config.order_constraints.push(OrderConstraint {
                            id: format!("ord-{}-after-{}", grp.group_name, fs.resource_name),
                            first: fs.resource_name.clone(),
                            first_action: "start".to_string(),
                            then: grp.group_name.clone(),
                            then_action: "start".to_string(),
                            kind: "Mandatory".to_string(),
                        });
                        config.colocation_constraints.push(ColocationConstraint {
                            id: format!("col-{}-with-{}", grp.group_name, fs.resource_name),
                            rsc: grp.group_name.clone(),
                            rsc_role: None,
                            with_rsc: fs.resource_name.clone(),
                            with_rsc_role: None,
                            score: "INFINITY".to_string(),
                        });
                    }
                } else {
                    // 그룹 없음 → 개별 systemd 리소스에 직접 연결
                    for svc in &svc_list {
                        config.order_constraints.push(OrderConstraint {
                            id: format!("ord-{}-after-{}", svc.resource_name, fs.resource_name),
                            first: fs.resource_name.clone(),
                            first_action: "start".to_string(),
                            then: svc.resource_name.clone(),
                            then_action: "start".to_string(),
                            kind: "Mandatory".to_string(),
                        });
                        config.colocation_constraints.push(ColocationConstraint {
                            id: format!("col-{}-with-{}", svc.resource_name, fs.resource_name),
                            rsc: svc.resource_name.clone(),
                            rsc_role: None,
                            with_rsc: fs.resource_name.clone(),
                            with_rsc_role: None,
                            score: "INFINITY".to_string(),
                        });
                    }
                }
            }
        } else {
            // FS 없음 — DRBD → Group 또는 DRBD → 개별 리소스
            let targets: Vec<String> = if !grp_list.is_empty() {
                grp_list.iter().map(|g| g.group_name.clone()).collect()
            } else {
                svc_list.iter().map(|s| s.resource_name.clone()).collect()
            };

            for target in targets {
                config.order_constraints.push(OrderConstraint {
                    id: format!("ord-{}-after-{}", target, drbd.clone_name),
                    first: drbd.clone_name.clone(),
                    first_action: "promote".to_string(),
                    then: target.clone(),
                    then_action: "start".to_string(),
                    kind: "Mandatory".to_string(),
                });
                config.colocation_constraints.push(ColocationConstraint {
                    id: format!("col-{}-with-{}", target, drbd.clone_name),
                    rsc: target.clone(),
                    rsc_role: None,
                    with_rsc: drbd.clone_name.clone(),
                    with_rsc_role: Some("Promoted".to_string()),
                    score: "INFINITY".to_string(),
                });
            }
        }
    }
}

/// CIB XML 조각 생성 (참고용)
pub fn generate_cib_xml_snippet(config: &PacemakerConfig) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" ?>\n");
    xml.push_str("<cib>\n");
    xml.push_str("  <configuration>\n");
    xml.push_str("    <resources>\n");

    for drbd in &config.drbd_resources {
        xml.push_str(&format!(
            "      <clone id=\"{clone}\" promotable=\"true\">\n",
            clone = drbd.clone_name
        ));
        xml.push_str(&format!(
            "        <primitive id=\"{name}\" class=\"ocf\" provider=\"linbit\" type=\"drbd\">\n",
            name = drbd.resource_name
        ));
        xml.push_str("          <instance_attributes>\n");
        xml.push_str(&format!(
            "            <nvpair name=\"drbd_resource\" value=\"{res}\"/>\n",
            res = drbd.drbd_resource_name
        ));
        xml.push_str("          </instance_attributes>\n");
        xml.push_str("        </primitive>\n");
        xml.push_str("      </clone>\n");
    }

    for fs in &config.fs_resources {
        xml.push_str(&format!(
            "      <primitive id=\"{name}\" class=\"ocf\" provider=\"heartbeat\" type=\"Filesystem\">\n",
            name = fs.resource_name
        ));
        xml.push_str("        <instance_attributes>\n");
        xml.push_str(&format!(
            "          <nvpair name=\"device\" value=\"{dev}\"/>\n",
            dev = fs.device
        ));
        xml.push_str(&format!(
            "          <nvpair name=\"directory\" value=\"{dir}\"/>\n",
            dir = fs.directory
        ));
        xml.push_str(&format!(
            "          <nvpair name=\"fstype\" value=\"{fs}\"/>\n",
            fs = fs.fstype
        ));
        xml.push_str("        </instance_attributes>\n");
        xml.push_str("      </primitive>\n");
    }

    for grp in &config.resource_groups {
        xml.push_str(&format!(
            "      <group id=\"{group}\">\n",
            group = grp.group_name
        ));
        for member in &grp.members {
            xml.push_str(&format!(
                "        <primitive id=\"{m}\"/>\n",
                m = member
            ));
        }
        xml.push_str("      </group>\n");
    }

    for svc in &config.systemd_resources {
        // 그룹에 속하지 않는 리소스만 표시
        let in_group = config.resource_groups.iter()
            .any(|g| g.members.contains(&svc.resource_name));
        if !in_group {
            xml.push_str(&format!(
                "      <primitive id=\"{name}\" class=\"systemd\" type=\"{unit}\"/>\n",
                name = svc.resource_name,
                unit = svc.systemd_unit,
            ));
        }
    }

    xml.push_str("    </resources>\n");
    xml.push_str("  </configuration>\n");
    xml.push_str("</cib>\n");
    xml
}

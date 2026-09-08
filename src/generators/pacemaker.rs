use crate::models::pacemaker::{
    ColocationConstraint, DrbdPacemakerResource, FsResource, LocationConstraint,
    OrderConstraint, PacemakerConfig, ResourceGroup, StonithDevice, SystemdResource,
};

/// STONITH 리소스 이름에 못 쓰는 문자를 `_`로 치환 (R02/R55 실측 규칙).
pub(crate) fn sanitize_stonith_id(node: &str) -> String {
    node.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

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

    // STONITH(fencing) 리소스 (PLAN.md A.5) — 참고용. 실제 멱등 배포는
    // Ansible 래퍼(routes/pacemaker.rs)에서 pcs stonith status 사전 캡처 +
    // is not search(...) 가드로 한다 (A.3와 동일한 방식).
    if !config.stonith_devices.is_empty() {
        lines.push("# ─── STONITH(fencing) 리소스 ─────────────────────────────────".to_string());
        for dev in &config.stonith_devices {
            lines.extend(generate_stonith_cmds(dev));
        }
        lines.push(String::new());
    }

    // DRBD Promotable Clone 리소스
    if !config.drbd_resources.is_empty() {
        lines.push("# ─── DRBD Promotable Clone 리소스 ───────────────────────────".to_string());
        // clone-max는 클러스터 노드 수를 따라야 한다 (PLAN.md A.2) — 3노드
        // 이상 클러스터에서 하드코딩된 값으로는 세 번째 이상 노드에 DRBD
        // clone이 배치되지 못하는 실제 버그가 있었다. 노드 목록이 비어
        // 있으면(사용자가 아직 클러스터 기본 섹션을 안 채운 경우) 이 앱의
        // 기본 대상인 3노드로 가정한다.
        let node_count = if config.cluster.nodes.is_empty() {
            3
        } else {
            config.cluster.nodes.len()
        };
        for drbd in &config.drbd_resources {
            lines.extend(generate_drbd_resource_cmds(drbd, node_count));
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

/// 단일 STONITH(fence_ipmilan) 생성 명령 (PLAN.md A.5, R02/R55 패턴 그대로).
fn generate_stonith_cmds(dev: &StonithDevice) -> Vec<String> {
    let id = format!("stonith-ipmi-{}", sanitize_stonith_id(&dev.node));
    let mut cmds = vec![
        format!("pcs stonith create {id} fence_ipmilan \\", id = id),
        format!("    pcmk_host_list={node} \\", node = dev.node),
        format!(
            "    ip={ip} user={user} password={pass} \\",
            ip = dev.ipmi_ip,
            user = dev.ipmi_user,
            pass = dev.ipmi_password
        ),
    ];
    let mut opts = vec![
        "lanplus=1".to_string(),
        "power_wait=5".to_string(),
        "pcmk_reboot_timeout=300".to_string(),
        "pcmk_monitor_timeout=60".to_string(),
        "pcmk_reboot_action=reboot".to_string(),
    ];
    opts.extend(dev.extra_opts.iter().cloned());
    cmds.push(format!("    {}", opts.join(" ")));
    cmds
}

pub(crate) fn generate_drbd_resource_cmds(drbd: &DrbdPacemakerResource, node_count: usize) -> Vec<String> {
    let mut cmds = Vec::new();

    cmds.push(format!(
        "pcs resource create {name} ocf:linbit:drbd \\",
        name = drbd.resource_name
    ));
    cmds.push(format!(
        "    drbd_resource={res} \\",
        res = drbd.drbd_resource_name
    ));
    // 가이드 1.1절 필수 op 세트 전부 명시 (PLAN.md A.2). notify/reload/monitor는
    // R11.pacemaker_drbd_resources.yml 실측상 리소스마다 값이 변하지 않아 상수로 둔다.
    cmds.push(format!(
        "    op demote  interval=0s timeout={t} \\",
        t = drbd.demote_timeout
    ));
    cmds.push("    op monitor interval=20s role=Promoted \\".to_string());
    cmds.push("    op monitor interval=20s role=Unpromoted \\".to_string());
    cmds.push("    op notify  interval=0s timeout=90s \\".to_string());
    cmds.push(format!(
        "    op promote interval=0s timeout={t} \\",
        t = drbd.promote_timeout
    ));
    cmds.push("    op reload  interval=0s timeout=30s \\".to_string());
    cmds.push(format!(
        "    op start   interval=0s timeout={t} \\",
        t = drbd.start_timeout
    ));
    cmds.push(format!(
        "    op stop    interval=0s timeout={t}",
        t = drbd.stop_timeout
    ));

    cmds.push(format!(
        "pcs resource promotable {name} \\",
        name = drbd.resource_name
    ));
    cmds.push("    promoted-max=1 promoted-node-max=1 \\".to_string());
    cmds.push(format!(
        "    clone-max={n} clone-node-max=1 \\",
        n = node_count
    ));
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

pub(crate) fn generate_fs_resource_cmds(fs: &FsResource) -> Vec<String> {
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

pub(crate) fn generate_systemd_resource_cmds(svc: &SystemdResource) -> Vec<String> {
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

pub(crate) fn generate_resource_group_cmd(grp: &ResourceGroup) -> String {
    format!(
        "pcs resource group add {group} {members}",
        group = grp.group_name,
        members = grp.members.join(" "),
    )
}

pub(crate) fn generate_order_constraint(ord: &OrderConstraint) -> String {
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

pub(crate) fn generate_colocation_constraint(col: &ColocationConstraint) -> String {
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

pub(crate) fn generate_location_constraint(loc: &LocationConstraint) -> String {
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

/// 롤링 유지보수(patch/reboot) 스크립트 생성 (PLAN.md A.7).
///
/// 가이드 5.1절 13단계 절차 그대로: versionlock 해제 → standby → 리소스
/// 이동 확인 → cluster stop → 업데이트 → 재부팅 → versionlock 재설정 →
/// cluster start → 재합류 확인 → unstandby → 최종 확인. 노드를 한 번에
/// 하나씩 처리하고, "Failed Resource Actions 없음"처럼 사람이 눈으로
/// 판단해야 하는 지점은 `read -p`로 일시정지해 확인을 받는다(완전
/// 무인 자동화는 이 단계에서는 위험 — 3.6절 STONITH 사고 참고).
/// `cluster.nodes`가 비어 있으면 빈 문자열을 반환한다.
pub fn generate_maintenance_script(config: &PacemakerConfig) -> String {
    if config.cluster.nodes.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push("#!/bin/bash".to_string());
    lines.push("# Pacemaker 클러스터 롤링 유지보수 스크립트 (가이드 5.1절 절차)".to_string());
    lines.push("# 노드를 한 번에 하나씩, 완전히 정상화를 확인한 뒤 다음 노드로 진행합니다.".to_string());
    lines.push("# set -e로 인해 어느 단계든 실패하면 즉시 중단되고 다음 노드로 넘어가지 않습니다.".to_string());
    lines.push("#".to_string());
    lines.push("# 알려진 공백(가이드 8.4절): Pacemaker가 관리하지 않는 podman 컨테이너".to_string());
    lines.push("# (Restart=always + WantedBy=multi-user.target로 구성된 Quadlet 유닛)는".to_string());
    lines.push("# 이 스크립트가 건드리지 않습니다 — 재부팅 전에 별도로".to_string());
    lines.push("# `systemctl stop <unit>`을 실행하세요. `podman kill`/`podman rm -f`로".to_string());
    lines.push("# 직접 종료하면 Restart=always 때문에 systemd가 즉시 재기동시킵니다.".to_string());
    lines.push(String::new());
    lines.push("set -euo pipefail".to_string());
    lines.push(String::new());

    let controller = &config.cluster.nodes[0].name;
    lines.push(format!("CONTROLLER=\"{}\"  # pcs 상태 조회는 이 노드를 통해 실행", controller));
    lines.push(format!(
        "NODES=({})",
        config.cluster.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(" ")
    ));
    lines.push("LOCK_PACKAGES=\"pacemaker corosync pcs\"".to_string());
    lines.push(String::new());
    lines.push("pcs_status() { ssh \"$CONTROLLER\" pcs status; }".to_string());
    lines.push(String::new());

    lines.push("for node in \"${NODES[@]}\"; do".to_string());
    lines.push("  echo \"=== [$node] 유지보수 시작 ===\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 1. 온라인 노드 수 확인 (과반수 유지 필요)\"".to_string());
    lines.push("  pcs_status".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 2. pacemaker/corosync/pcs versionlock 해제\"".to_string());
    lines.push("  ssh \"$node\" \"dnf versionlock delete $LOCK_PACKAGES\" || true".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 3. standby\"".to_string());
    lines.push("  ssh \"$CONTROLLER\" \"pcs node standby $node\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 4. 리소스가 다른 노드로 모두 이동했는지 확인\"".to_string());
    lines.push("  pcs_status".to_string());
    lines.push("  read -r -p \"[$node] Failed Resource Actions 없이 리소스가 이동했습니까? 계속하려면 Enter, 중단하려면 Ctrl+C: \"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 5. 클러스터 멤버십에서 완전히 이탈 (STONITH 오발동 방지)\"".to_string());
    lines.push("  ssh \"$node\" \"pcs cluster stop\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 6. 다른 노드 관점에서 Offline 확인\"".to_string());
    lines.push("  pcs_status".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 7. 패키지 업데이트 (커널/DRBD 모듈 포함)\"".to_string());
    lines.push("  ssh \"$node\" \"dnf update -y\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 8. 재부팅 후 SSH 재접속 대기\"".to_string());
    lines.push("  ssh \"$node\" \"reboot\" || true".to_string());
    lines.push("  until ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no \"$node\" true 2>/dev/null; do sleep 5; done".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 9. versionlock 재설정\"".to_string());
    lines.push("  ssh \"$node\" \"dnf versionlock add $LOCK_PACKAGES\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 10. 클러스터 재합류\"".to_string());
    lines.push("  ssh \"$node\" \"pcs cluster start\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 11. Online 재합류 확인\"".to_string());
    lines.push("  pcs_status".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 12. unstandby\"".to_string());
    lines.push("  ssh \"$CONTROLLER\" \"pcs node unstandby $node\"".to_string());
    lines.push(String::new());
    lines.push("  echo \"[$node] 13. 최종 확인\"".to_string());
    lines.push("  pcs_status".to_string());
    lines.push("  read -r -p \"[$node] Failed Resource Actions 없습니까? 다음 노드로 진행하려면 Enter, 중단하려면 Ctrl+C: \"".to_string());
    lines.push(String::new());
    lines.push("  echo \"=== [$node] 유지보수 완료 ===\"".to_string());
    lines.push("done".to_string());
    lines.push(String::new());
    lines.push("echo '✅ 모든 노드 유지보수 완료'".to_string());

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pacemaker::{ClusterConfig, ClusterNode};

    fn config_with_nodes(node_count: usize) -> PacemakerConfig {
        let mut config = PacemakerConfig::default();
        config.cluster = ClusterConfig {
            nodes: (0..node_count)
                .map(|i| ClusterNode { name: format!("node{}", i + 1), id: (i + 1) as u32 })
                .collect(),
            ..ClusterConfig::default()
        };
        config.drbd_resources.push(DrbdPacemakerResource::default());
        config
    }

    #[test]
    fn clone_max_follows_cluster_node_count() {
        let script = generate_pcs_script(&config_with_nodes(3));
        assert!(script.contains("clone-max=3 clone-node-max=1"));
        assert!(!script.contains("clone-max=2 "));
    }

    #[test]
    fn clone_max_defaults_to_three_when_nodes_unset() {
        // cluster.nodes가 비어 있으면(사용자가 클러스터 기본 섹션을 아직
        // 안 채운 경우) 이 앱의 기본 대상인 3노드로 가정해야 한다.
        let script = generate_pcs_script(&config_with_nodes(0));
        assert!(script.contains("clone-max=3 clone-node-max=1"));
    }

    #[test]
    fn drbd_resource_emits_full_op_set_with_configured_timeouts() {
        let mut config = config_with_nodes(3);
        config.drbd_resources[0].promote_timeout = "120s".to_string();
        config.drbd_resources[0].demote_timeout = "120s".to_string();
        config.drbd_resources[0].start_timeout = "240s".to_string();
        config.drbd_resources[0].stop_timeout = "180s".to_string();

        let script = generate_pcs_script(&config);

        assert!(script.contains("op demote  interval=0s timeout=120s"));
        assert!(script.contains("op monitor interval=20s role=Promoted"));
        assert!(script.contains("op monitor interval=20s role=Unpromoted"));
        assert!(script.contains("op notify  interval=0s timeout=90s"));
        assert!(script.contains("op promote interval=0s timeout=120s"));
        assert!(script.contains("op reload  interval=0s timeout=30s"));
        assert!(script.contains("op start   interval=0s timeout=240s"));
        assert!(script.contains("op stop    interval=0s timeout=180s"));
    }

    #[test]
    fn drbd_resource_default_timeouts_match_preset_table() {
        let script = generate_pcs_script(&config_with_nodes(3));
        assert!(script.contains("op demote  interval=0s timeout=90s"));
        assert!(script.contains("op promote interval=0s timeout=90s"));
        assert!(script.contains("op start   interval=0s timeout=240s"));
        assert!(script.contains("op stop    interval=0s timeout=100s"));
    }
}

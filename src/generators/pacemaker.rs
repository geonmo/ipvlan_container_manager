use crate::models::pacemaker::{
    ColocationConstraint, DrbdPacemakerResource, LocationConstraint, OrderConstraint,
    PacemakerConfig, SystemdResource,
};

/// pcs コマンド スクリプト 전체 생성
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

    // Systemd (Quadlet) 리소스
    if !config.systemd_resources.is_empty() {
        lines.push("# ─── Quadlet Systemd 리소스 ──────────────────────────────────".to_string());
        for svc in &config.systemd_resources {
            lines.extend(generate_systemd_resource_cmds(svc));
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

    // 기본 DRBD 리소스 에이전트 등록
    cmds.push(format!(
        "pcs resource create {name} ocf:linbit:drbd \\",
        name = drbd.resource_name
    ));
    cmds.push(format!(
        "    drbd_resource={res} \\",
        res = drbd.drbd_resource_name
    ));
    cmds.push(format!(
        "    op monitor interval=29s role=Promoted \\",
    ));
    cmds.push(format!(
        "    op monitor interval=31s role=Unpromoted"
    ));

    // Promotable Clone
    cmds.push(format!(
        "pcs resource promotable {name} \\",
        name = drbd.resource_name
    ));
    cmds.push(format!(
        "    promoted-max=1 promoted-node-max=1 \\",
    ));
    cmds.push(format!(
        "    clone-max=2 clone-node-max=1 \\",
    ));
    cmds.push(format!(
        "    notify={notify} \\",
        notify = drbd.notify
    ));
    cmds.push(format!(
        "    id={clone_name}",
        clone_name = drbd.clone_name
    ));

    // 선호 Primary 노드
    if let Some(node) = &drbd.target_role_master_node {
        cmds.push(format!(
            "pcs constraint location {clone} prefers {node}=200",
            clone = drbd.clone_name,
            node = node
        ));
    }

    cmds
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
/// DRBD Promoted → Quadlet 서비스 시작 순서 보장
pub fn generate_default_constraints(config: &mut PacemakerConfig) {
    for drbd in &config.drbd_resources {
        for svc in &config.systemd_resources {
            // DRBD가 Promoted 상태가 된 후에 서비스 시작
            let order_id = format!("ord-{}-after-{}", svc.resource_name, drbd.clone_name);
            config.order_constraints.push(OrderConstraint {
                id: order_id,
                first: drbd.clone_name.clone(),
                first_action: "promote".to_string(),
                then: svc.resource_name.clone(),
                then_action: "start".to_string(),
                kind: "Mandatory".to_string(),
            });

            // 서비스는 DRBD Promoted 노드와 같은 노드에서 실행
            let col_id = format!("col-{}-with-{}", svc.resource_name, drbd.clone_name);
            config.colocation_constraints.push(ColocationConstraint {
                id: col_id,
                rsc: svc.resource_name.clone(),
                rsc_role: None,
                with_rsc: drbd.clone_name.clone(),
                with_rsc_role: Some("Promoted".to_string()),
                score: "INFINITY".to_string(),
            });
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

    for svc in &config.systemd_resources {
        xml.push_str(&format!(
            "      <primitive id=\"{name}\" class=\"systemd\" type=\"{unit}\"/>\n",
            name = svc.resource_name,
            unit = svc.systemd_unit,
        ));
    }

    xml.push_str("    </resources>\n");
    xml.push_str("  </configuration>\n");
    xml.push_str("</cib>\n");
    xml
}

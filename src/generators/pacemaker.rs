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
            let id = format!("stonith-ipmi-{}", sanitize_stonith_id(&dev.node));
            lines.extend(guard_resource(&id, generate_stonith_cmds(dev)));
        }
        lines.push(String::new());
    }

    // DRBD Promotable Clone 리소스
    if !config.drbd_resources.is_empty() {
        // on-fail=fence 는 STONITH 가 켜져 있어야 의미가 있다. pcs 는 이 조합을
        // 거부하지 않고(실측 rc=0) Pacemaker 가 런타임에 조용히 stop 으로
        // 격하시킨다 — 사용자는 fence 를 골랐는데 펜싱이 일어나지 않는다.
        let fence_without_stonith: Vec<&str> = config
            .drbd_resources
            .iter()
            .filter(|d| d.on_fail.trim() == "fence")
            .map(|d| d.resource_name.as_str())
            .collect();
        if !fence_without_stonith.is_empty() && !config.cluster.stonith_enabled {
            lines.push("# ⚠️  주의: 아래 리소스에 on-fail=fence 가 지정돼 있지만".to_string());
            lines.push("#     stonith-enabled=false 입니다. Pacemaker 는 이 조합에서".to_string());
            lines.push("#     on-fail 을 조용히 stop 으로 격하시키므로 펜싱이 일어나지".to_string());
            lines.push("#     않습니다. STONITH 를 켜거나 on-fail 을 바꾸세요.".to_string());
            lines.push(format!("#     해당 리소스: {}", fence_without_stonith.join(", ")));
        }
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
            lines.extend(guard_resource(
                &drbd.resource_name,
                generate_drbd_resource_cmds(drbd, node_count),
            ));
        }
        lines.push(String::new());
    }

    // Filesystem 리소스 (DRBD 볼륨 위 마운트)
    if !config.fs_resources.is_empty() {
        lines.push("# ─── Filesystem 리소스 (ocf:heartbeat:Filesystem) ───────────".to_string());
        lines.push("# DRBD가 Primary로 승격된 후 볼륨을 마운트합니다.".to_string());
        for fs in &config.fs_resources {
            lines.extend(guard_resource(&fs.resource_name, generate_fs_resource_cmds(fs)));
        }
        lines.push(String::new());
    }

    // Systemd (Quadlet) 리소스
    if !config.systemd_resources.is_empty() {
        lines.push("# ─── Quadlet Systemd 리소스 (Pod / Container) ───────────────".to_string());
        for svc in &config.systemd_resources {
            lines.extend(guard_resource(
                &svc.resource_name,
                generate_systemd_resource_cmds(svc),
            ));
        }
        lines.push(String::new());
    }

    // 리소스 그룹 (Pod + Container들을 순서대로 묶음)
    if !config.resource_groups.is_empty() {
        lines.push("# ─── 리소스 그룹 (Pod → Container 순서 보장) ───────────────".to_string());
        lines.push("# 그룹 내 start: members 순서대로, stop: 역순 자동 처리".to_string());
        lines.push("# (pcs resource group add 는 자체적으로 멱등이라 가드를 두지 않는다)".to_string());
        for grp in &config.resource_groups {
            lines.push(generate_resource_group_cmd(grp));
        }
        lines.push(String::new());
    }

    // Order 제약조건
    if !config.order_constraints.is_empty() {
        lines.push("# ─── Order 제약조건 ──────────────────────────────────────────".to_string());
        for ord in &config.order_constraints {
            lines.extend(guard_constraint(&ord.id, generate_order_constraint(ord)));
        }
        lines.push(String::new());
    }

    // Colocation 제약조건
    if !config.colocation_constraints.is_empty() {
        lines.push("# ─── Colocation 제약조건 ────────────────────────────────────".to_string());
        for col in &config.colocation_constraints {
            lines.extend(guard_constraint(&col.id, generate_colocation_constraint(col)));
        }
        lines.push(String::new());
    }

    // Location 제약조건 (선호 노드)
    if !config.location_constraints.is_empty() {
        lines.push("# ─── Location 제약조건 (선호 노드) ─────────────────────────".to_string());
        for loc in &config.location_constraints {
            lines.extend(guard_constraint(&loc.id, generate_location_constraint(loc)));
        }
        lines.push(String::new());
    }

    lines.push("echo '✅ Pacemaker 설정 완료'".to_string());
    lines.join("\n")
}

/// 리소스가 이미 있으면 건너뛰도록 `pcs resource create` 묶음을 감싼다.
///
/// 생성 스크립트는 `set -euo pipefail` 이라 `pcs resource create` 가
/// "already exists" 로 실패하면 **거기서 멈춘다**. 그런데 그 앞 명령들은
/// 이미 적용된 뒤라, 부분 적용 상태로 끝나고 사용자는 어디까지 됐는지
/// 직접 확인해야 한다. `pcs resource config <id>` 는 없으면 rc=1, 있으면
/// rc=0 이므로(실측) 이걸로 가드한다.
fn guard_resource(id: &str, body: Vec<String>) -> Vec<String> {
    let mut out = vec![format!(
        "if ! pcs resource config {id} >/dev/null 2>&1; then",
        id = shell_quote(id)
    )];
    for line in body {
        out.push(format!("  {}", line));
    }
    out.push(format!("else"));
    out.push(format!(
        "  echo \"  - {id} 는 이미 있어 건너뜁니다\"",
        id = id
    ));
    out.push("fi".to_string());
    out
}

/// 제약조건이 이미 있으면 건너뛴다. `pcs constraint --full` 출력에
/// `(id: <id>)` 형태로 나오는 것을 확인했다(실측).
fn guard_constraint(id: &str, cmd: String) -> Vec<String> {
    vec![
        format!(
            "if ! pcs constraint --full 2>/dev/null | grep -q \"(id: {id})\"; then",
            id = id
        ),
        format!("  {}", cmd),
        "else".to_string(),
        format!("  echo \"  - 제약조건 {id} 는 이미 있어 건너뜁니다\"", id = id),
        "fi".to_string(),
    ]
}

/// bash 작은따옴표로 안전하게 감싼다.
///
/// 생성물은 `set -euo pipefail` 이 걸린 bash 스크립트이고, IPMI 비밀번호 같은
/// 값은 사용자 입력이라 공백·`$`·`;`·따옴표가 들어올 수 있다. 그대로 넣으면
/// 명령이 잘리거나(공백) 의도치 않은 명령이 실행된다(`;`, `$(...)`).
/// 작은따옴표 안에서는 `'` 만 특별하므로 `'\''` 로 끊어 이어 붙인다.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 단일 STONITH(fence_ipmilan) 생성 명령 (PLAN.md A.5, R02/R55 패턴 그대로).
fn generate_stonith_cmds(dev: &StonithDevice) -> Vec<String> {
    let id = format!("stonith-ipmi-{}", sanitize_stonith_id(&dev.node));
    let mut cmds = vec![
        format!("pcs stonith create {id} fence_ipmilan \\", id = id),
        format!("    pcmk_host_list={node} \\", node = shell_quote(&dev.node)),
        format!(
            "    ip={ip} user={user} password={pass} \\",
            ip = shell_quote(&dev.ipmi_ip),
            user = shell_quote(&dev.ipmi_user),
            pass = shell_quote(&dev.ipmi_password)
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
    // 역할별 monitor 두 개는 **서로 다른 interval** 이어야 한다.
    // pcs(0.11.11 실측)는 역할이 달라도 같은 interval 의 monitor 를 두 번
    // 지정하면 거부한다:
    //   Error: multiple specification of the same operation with the same
    //   interval: monitor with intervals 20s, 20s
    // 예전에는 양쪽 다 20s 로 내보내서 생성된 pcs 스크립트가 아예 실행되지
    // 않았다. LINBIT 예제 관행대로 서로 다른(그리고 서로 배수가 아닌)
    // 값을 쓴다 — 두 monitor 가 매번 같은 시점에 겹치지 않게 한다.
    //
    // on-fail 은 Promoted 역할 monitor 에 붙인다. UI 드롭다운이 제공하는
    // 값 집합(fence/block/stop/ignore/demote)이 정확히 이 op 에서만 전부
    // 유효하기 때문이다 — Pacemaker 에서 `demote` 는 promote 액션과
    // `role=Promoted` 반복 monitor 에만 허용된다. 값이 비어 있으면
    // Pacemaker 기본값(restart)에 맡긴다.
    let on_fail = drbd.on_fail.trim();
    if on_fail.is_empty() {
        cmds.push(format!(
            "    op monitor interval={i} role=Promoted \\",
            i = drbd.monitor_interval_promoted
        ));
    } else {
        cmds.push(format!(
            "    op monitor interval={i} role=Promoted on-fail={f} \\",
            i = drbd.monitor_interval_promoted,
            f = on_fail
        ));
    }
    cmds.push(format!(
        "    op monitor interval={i} role=Unpromoted \\",
        i = drbd.monitor_interval_unpromoted
    ));
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
    // pcs 문법은 역할이 **리소스 id 앞에** 온다:
    //   colocation add [<role>] <source> with [<role>] <target> [score] [options]
    //
    // 예전에는 `... with <target> role=Promoted` 처럼 뒤에 붙였는데, pcs 는
    // 그걸 옵션으로 받아 스키마에 맞지 않는 CIB 를 만들고 거부한다
    // (실측: "Error: Unable to update cib / Update does not conform to the
    // configured schema"). 그 결과 콜로케이션이 통째로 빠져서 Filesystem 이
    // DRBD Primary 가 아닌 노드에서 시작하려다 마운트 실패까지 이어졌다.
    let rsc_role = col
        .rsc_role
        .as_deref()
        .map(|r| format!("{} ", r))
        .unwrap_or_default();
    let with_role = col
        .with_rsc_role
        .as_deref()
        .map(|r| format!("{} ", r))
        .unwrap_or_default();

    format!(
        "pcs constraint colocation add {rsc_role}{rsc} with {with_role}{with} score={score} id={id}",
        rsc_role = rsc_role,
        rsc = col.rsc,
        with_role = with_role,
        with = col.with_rsc,
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

/// 이 설정이 만든 Pacemaker 리소스/제약조건을 제거하는 스크립트 생성.
///
/// 생성 스크립트만 있고 되돌릴 방법이 없으면, 설정을 바꿔 다시 적용하려 할 때
/// 사용자가 CIB 를 직접 들여다보며 무엇을 지울지 찾아야 한다. 이 앱의 목적이
/// 그 부담을 없애는 것이므로 짝이 되는 teardown 을 함께 낸다.
///
/// **의존 역순으로 지운다**: 제약조건 → 그룹 → systemd/FS → DRBD clone →
/// STONITH. 제약조건이 남아 있으면 리소스 삭제가 막히고, 그룹이 남아 있으면
/// 멤버 삭제가 막힌다.
///
/// 리소스가 없을 때도 실패하지 않아야 재실행할 수 있으므로 각 삭제를
/// 존재 확인으로 감싼다. DRBD 볼륨 자체(LINSTOR 리소스나 .res 파일)는
/// **건드리지 않는다** — Pacemaker 등록만 해제한다.
pub fn generate_teardown_script(config: &PacemakerConfig) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("#!/bin/bash".to_string());
    lines.push("# Pacemaker 리소스 해제 스크립트 (생성 스크립트의 짝)".to_string());
    lines.push("#".to_string());
    lines.push("# 이 앱이 등록한 Pacemaker 리소스와 제약조건만 제거합니다.".to_string());
    lines.push("# DRBD 볼륨의 데이터, LINSTOR 리소스, .res 파일은 건드리지 않습니다".to_string());
    lines.push("# — Pacemaker 관리에서만 빼냅니다.".to_string());
    lines.push("#".to_string());
    lines.push("# 의존 역순으로 지웁니다: 제약조건 → 그룹 → 서비스/FS → DRBD → STONITH".to_string());
    lines.push("# 없는 항목은 건너뛰므로 여러 번 실행해도 안전합니다.".to_string());
    lines.push(String::new());
    lines.push("set -euo pipefail".to_string());
    lines.push(String::new());

    let del_constraint = |id: &str| -> Vec<String> {
        vec![
            format!(
                "if pcs constraint --full 2>/dev/null | grep -q \"(id: {id})\"; then",
                id = id
            ),
            format!("  pcs constraint delete {}", shell_quote(id)),
            "else".to_string(),
            format!("  echo \"  - 제약조건 {id} 없음 (건너뜀)\"", id = id),
            "fi".to_string(),
        ]
    };
    let del_resource = |id: &str| -> Vec<String> {
        vec![
            format!("if pcs resource config {} >/dev/null 2>&1; then", shell_quote(id)),
            // --force 로 정지 대기를 건너뛰지 않는다. 정상 정지시켜야
            // Filesystem 이 언마운트되고 DRBD 가 Secondary 로 내려간다.
            format!("  pcs resource delete {}", shell_quote(id)),
            "else".to_string(),
            format!("  echo \"  - 리소스 {id} 없음 (건너뜀)\"", id = id),
            "fi".to_string(),
        ]
    };

    if !config.order_constraints.is_empty()
        || !config.colocation_constraints.is_empty()
        || !config.location_constraints.is_empty()
    {
        lines.push("# ─── 1. 제약조건 제거 ────────────────────────────────────────".to_string());
        for c in &config.location_constraints {
            lines.extend(del_constraint(&c.id));
        }
        for c in &config.colocation_constraints {
            lines.extend(del_constraint(&c.id));
        }
        for c in &config.order_constraints {
            lines.extend(del_constraint(&c.id));
        }
        lines.push(String::new());
    }

    if !config.resource_groups.is_empty() {
        lines.push("# ─── 2. 리소스 그룹 해체 ─────────────────────────────────────".to_string());
        lines.push("# 그룹을 지우면 멤버는 남는다(아래에서 개별 삭제).".to_string());
        for grp in &config.resource_groups {
            lines.extend(del_resource(&grp.group_name));
        }
        lines.push(String::new());
    }

    if !config.systemd_resources.is_empty() {
        lines.push("# ─── 3. Systemd(Quadlet) 리소스 제거 ─────────────────────────".to_string());
        for svc in &config.systemd_resources {
            lines.extend(del_resource(&svc.resource_name));
        }
        lines.push(String::new());
    }

    if !config.fs_resources.is_empty() {
        lines.push("# ─── 4. Filesystem 리소스 제거 (언마운트됨) ──────────────────".to_string());
        for fs in &config.fs_resources {
            lines.extend(del_resource(&fs.resource_name));
        }
        lines.push(String::new());
    }

    if !config.drbd_resources.is_empty() {
        lines.push("# ─── 5. DRBD Promotable Clone 제거 ───────────────────────────".to_string());
        lines.push("# clone id 를 지우면 안에 든 primitive 도 함께 사라진다.".to_string());
        for drbd in &config.drbd_resources {
            lines.extend(del_resource(&drbd.clone_name));
        }
        lines.push(String::new());
    }

    if !config.stonith_devices.is_empty() {
        lines.push("# ─── 6. STONITH 리소스 제거 ──────────────────────────────────".to_string());
        for dev in &config.stonith_devices {
            let id = format!("stonith-ipmi-{}", sanitize_stonith_id(&dev.node));
            lines.extend(del_resource(&id));
        }
        lines.push(String::new());
    }

    lines.push("# 실패 기록이 남아 있으면 정리한다 (없으면 no-op)".to_string());
    lines.push("pcs resource cleanup >/dev/null 2>&1 || true".to_string());
    lines.push(String::new());
    lines.push("echo '✅ Pacemaker 리소스 해제 완료'".to_string());
    lines.push("pcs resource status || true".to_string());

    lines.join("\n")
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
        assert!(script.contains("op monitor interval=29s role=Promoted"));
        assert!(script.contains("op monitor interval=31s role=Unpromoted"));
        assert!(script.contains("op notify  interval=0s timeout=90s"));
        assert!(script.contains("op promote interval=0s timeout=120s"));
        assert!(script.contains("op reload  interval=0s timeout=30s"));
        assert!(script.contains("op start   interval=0s timeout=240s"));
        assert!(script.contains("op stop    interval=0s timeout=180s"));
    }

    /// UI가 받는 on_fail 값이 pcs 명령에 실제로 나타나야 한다.
    /// (이전에는 폼 → 모델까지만 전달되고 생성기가 통째로 무시했다.)
    /// UI는 FS 없는 DRBD 그룹에서 `after_fs: ""`를 보낸다. 라우터가 이걸
    /// None으로 정규화하지 않으면 여기 필터가 `Some(fs_name)`에도
    /// `is_none()`에도 걸리지 않아, **그룹 대신 그룹 멤버 개별 리소스**에
    /// colocation INFINITY가 붙는다 (그룹이 쪼개진다).
    #[test]
    fn group_without_after_fs_is_constrained_as_a_group_not_as_members() {
        let mut config = config_with_nodes(3);
        config.fs_resources.push(FsResource {
            resource_name: "fs-r0".to_string(),
            drbd_clone_name: config.drbd_resources[0].clone_name.clone(),
            ..FsResource::default()
        });
        config.systemd_resources.push(SystemdResource {
            resource_name: "svc-app".to_string(),
            systemd_unit: "myapp.service".to_string(),
            ..SystemdResource::default()
        });
        config.resource_groups.push(ResourceGroup {
            group_name: "grp-app".to_string(),
            members: vec!["svc-app".to_string()],
            after_fs: None, // 라우터가 "" → None 으로 정규화한 결과
        });

        generate_default_constraints(&mut config);

        assert!(config
            .colocation_constraints
            .iter()
            .any(|c| c.rsc == "grp-app" && c.with_rsc == "fs-r0"));
        // 그룹 멤버에 직접 제약조건이 붙으면 안 된다.
        assert!(!config.colocation_constraints.iter().any(|c| c.rsc == "svc-app"));
        assert!(!config.order_constraints.iter().any(|o| o.then == "svc-app"));
    }

    /// pcs(0.11.11 실측)는 역할이 달라도 **같은 interval 의 monitor 를 두 번**
    /// 지정하면 명령 전체를 거부한다:
    ///   Error: multiple specification of the same operation with the same
    ///   interval: monitor with intervals 20s, 20s
    /// 예전에는 양쪽 다 20s 였고, 그래서 생성된 pcs 스크립트가 실행 자체가
    /// 되지 않았다.
    /// on-fail=fence 는 STONITH 가 켜져 있어야 의미가 있다. pcs 는 이 조합을
    /// 거부하지 않고 Pacemaker 가 런타임에 조용히 stop 으로 격하시키므로,
    /// 생성물에 경고를 남기지 않으면 사용자가 알 방법이 없다.
    /// pcs 는 콜로케이션의 역할을 **리소스 id 앞에** 받는다:
    ///   colocation add [<role>] <source> with [<role>] <target>
    /// 뒤에 `role=` 로 붙이면 옵션으로 해석돼 스키마에 맞지 않는 CIB 가 되고
    /// pcs 가 거부한다(실측). 그러면 콜로케이션이 빠져 Filesystem 이 DRBD
    /// Primary 가 아닌 노드에서 시작하려다 마운트 실패로 이어진다.
    fn full_config() -> PacemakerConfig {
        let mut c = config_with_nodes(3);
        c.fs_resources.push(FsResource {
            drbd_clone_name: c.drbd_resources[0].clone_name.clone(),
            ..FsResource::default()
        });
        c.systemd_resources.push(SystemdResource {
            resource_name: "svc-app".to_string(),
            systemd_unit: "app.service".to_string(),
            ..SystemdResource::default()
        });
        c.resource_groups.push(ResourceGroup {
            group_name: "grp-app".to_string(),
            members: vec!["svc-app".to_string()],
            after_fs: Some("fs-r0".to_string()),
        });
        c.stonith_devices.push(StonithDevice {
            node: "node1".to_string(),
            ipmi_ip: "10.0.0.1".to_string(),
            ipmi_user: "admin".to_string(),
            ipmi_password: "secret".to_string(),
            extra_opts: vec![],
        });
        generate_default_constraints(&mut c);
        c
    }

    /// 생성 스크립트는 `set -euo pipefail` 이라 `pcs resource create` 가
    /// "already exists" 로 실패하면 거기서 멈춘다. 그 앞 명령들은 이미
    /// 적용된 뒤라 부분 적용 상태로 끝난다. 모든 생성을 존재 확인으로
    /// 감싸야 재실행이 안전하다.
    #[test]
    fn every_resource_create_is_guarded() {
        let script = generate_pcs_script(&full_config());

        for line in script.lines().filter(|l| l.trim_start().starts_with("pcs resource create")) {
            // 가드 블록 안이라면 두 칸 들여쓰기돼 있다
            assert!(
                line.starts_with("  "),
                "가드되지 않은 생성 명령: {}",
                line
            );
        }
        assert!(script.contains("if ! pcs resource config"));
        assert!(script.contains("이미 있어 건너뜁니다"));
    }

    #[test]
    fn every_constraint_is_guarded() {
        let script = generate_pcs_script(&full_config());
        for line in script
            .lines()
            .filter(|l| l.trim_start().starts_with("pcs constraint"))
        {
            assert!(line.starts_with("  "), "가드되지 않은 제약조건: {}", line);
        }
        assert!(script.contains("pcs constraint --full 2>/dev/null | grep -q"));
    }

    /// teardown 은 의존 역순이어야 한다. 제약조건이 남아 있으면 리소스 삭제가
    /// 막히고, 그룹이 남아 있으면 멤버 삭제가 막힌다.
    #[test]
    fn teardown_removes_in_reverse_dependency_order() {
        let config = full_config();
        let script = generate_teardown_script(&config);

        let pos = |needle: &str| script.find(needle).unwrap_or_else(|| panic!("없음: {}", needle));
        let constraints = pos("1. 제약조건 제거");
        let groups = pos("2. 리소스 그룹 해체");
        let systemd = pos("3. Systemd");
        let fs = pos("4. Filesystem");
        let drbd = pos("5. DRBD");
        let stonith = pos("6. STONITH");

        assert!(constraints < groups);
        assert!(groups < systemd);
        assert!(systemd < fs);
        assert!(fs < drbd);
        assert!(drbd < stonith);

        // clone id 를 지워야 primitive 까지 사라진다
        assert!(script.contains(&config.drbd_resources[0].clone_name));
    }

    /// teardown 도 재실행 가능해야 한다 — 없는 항목에서 멈추면 안 된다.
    #[test]
    fn teardown_skips_missing_items() {
        let script = generate_teardown_script(&full_config());
        assert!(script.contains("없음 (건너뜀)"));
        assert!(script.contains("if pcs resource config"));
    }

    /// teardown 은 Pacemaker 등록만 해제한다. DRBD 볼륨/LINSTOR 리소스를
    /// 지우면 데이터가 날아간다.
    #[test]
    fn teardown_does_not_touch_storage() {
        let script = generate_teardown_script(&full_config());
        for forbidden in ["drbdadm", "linstor ", "lvremove", "wipefs", "mkfs"] {
            assert!(
                !script.contains(forbidden),
                "teardown 이 스토리지를 건드린다: {}",
                forbidden
            );
        }
    }

    #[test]
    fn colocation_role_comes_before_the_resource_id() {
        let cmd = generate_colocation_constraint(&ColocationConstraint {
            id: "col-1".to_string(),
            rsc: "fs-r0".to_string(),
            rsc_role: None,
            with_rsc: "drbd-r0-clone".to_string(),
            with_rsc_role: Some("Promoted".to_string()),
            score: "INFINITY".to_string(),
        });

        assert_eq!(
            cmd,
            "pcs constraint colocation add fs-r0 with Promoted drbd-r0-clone score=INFINITY id=col-1"
        );
        // 옵션 형태로 나가면 안 된다
        assert!(!cmd.contains("role="));
    }

    #[test]
    fn colocation_supports_role_on_both_sides() {
        let cmd = generate_colocation_constraint(&ColocationConstraint {
            id: "col-2".to_string(),
            rsc: "a".to_string(),
            rsc_role: Some("Started".to_string()),
            with_rsc: "b".to_string(),
            with_rsc_role: Some("Promoted".to_string()),
            score: "INFINITY".to_string(),
        });
        assert_eq!(
            cmd,
            "pcs constraint colocation add Started a with Promoted b score=INFINITY id=col-2"
        );
    }

    #[test]
    fn colocation_without_roles_is_unchanged() {
        let cmd = generate_colocation_constraint(&ColocationConstraint {
            id: "col-3".to_string(),
            rsc: "a".to_string(),
            rsc_role: None,
            with_rsc: "b".to_string(),
            with_rsc_role: None,
            score: "INFINITY".to_string(),
        });
        assert_eq!(
            cmd,
            "pcs constraint colocation add a with b score=INFINITY id=col-3"
        );
    }

    #[test]
    fn warns_when_on_fail_fence_without_stonith() {
        let mut config = config_with_nodes(3);
        config.cluster.stonith_enabled = false;
        config.drbd_resources[0].on_fail = "fence".to_string();
        let script = generate_pcs_script(&config);

        assert!(script.contains("stonith-enabled=false"));
        assert!(script.contains("on-fail 을 조용히 stop 으로 격하"));
        assert!(script.contains(&config.drbd_resources[0].resource_name));
    }

    #[test]
    fn no_warning_when_stonith_enabled() {
        let mut config = config_with_nodes(3);
        config.cluster.stonith_enabled = true;
        config.drbd_resources[0].on_fail = "fence".to_string();
        assert!(!generate_pcs_script(&config).contains("조용히 stop 으로 격하"));
    }

    #[test]
    fn no_warning_when_on_fail_is_not_fence() {
        let mut config = config_with_nodes(3);
        config.cluster.stonith_enabled = false;
        config.drbd_resources[0].on_fail = "restart".to_string();
        assert!(!generate_pcs_script(&config).contains("조용히 stop 으로 격하"));
    }

    #[test]
    fn role_monitors_must_use_different_intervals() {
        let script = generate_pcs_script(&config_with_nodes(3));

        let intervals: Vec<&str> = script
            .lines()
            .filter(|l| l.contains("op monitor") && l.contains("role="))
            .filter_map(|l| {
                l.split("interval=")
                    .nth(1)?
                    .split_whitespace()
                    .next()
            })
            .collect();

        assert_eq!(intervals.len(), 2, "역할별 monitor 가 두 개여야 한다");
        assert_ne!(
            intervals[0], intervals[1],
            "Promoted/Unpromoted monitor 의 interval 이 같으면 pcs 가 거부한다: {:?}",
            intervals
        );
    }

    /// 사용자가 폼에서 같은 값을 넣어도 생성물이 깨지지 않는지는 별개 문제다.
    /// 최소한 기본값끼리는 절대 겹치지 않아야 한다.
    #[test]
    fn default_monitor_intervals_are_not_multiples_of_each_other() {
        let d = DrbdPacemakerResource::default();
        let p: u32 = d.monitor_interval_promoted.trim_end_matches('s').parse().unwrap();
        let u: u32 = d.monitor_interval_unpromoted.trim_end_matches('s').parse().unwrap();
        assert_ne!(p, u);
        assert!(p % u != 0 && u % p != 0, "서로 배수면 주기적으로 겹친다: {}/{}", p, u);
    }

    #[test]
    fn drbd_on_fail_is_attached_to_the_promoted_role_monitor() {
        let mut config = config_with_nodes(3);
        config.drbd_resources[0].on_fail = "fence".to_string();
        let script = generate_pcs_script(&config);

        assert!(script.contains("op monitor interval=29s role=Promoted on-fail=fence"));
        // Unpromoted monitor에는 붙지 않는다 — `demote` 같은 값이 그쪽에서는
        // 유효하지 않아 pcs가 거부한다.
        assert!(script.contains("op monitor interval=31s role=Unpromoted \\"));
        assert!(!script.contains("role=Unpromoted on-fail"));
    }

    #[test]
    fn drbd_on_fail_is_omitted_when_unset_so_pacemaker_default_applies() {
        let mut config = config_with_nodes(3);
        config.drbd_resources[0].on_fail = String::new();
        let script = generate_pcs_script(&config);

        assert!(script.contains("op monitor interval=29s role=Promoted \\"));
        assert!(!script.contains("on-fail="));
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

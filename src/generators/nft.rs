use crate::models::nft::{NftPolicy, NftSubnetGroup, NftServiceDef};

/// 서비스/그룹 이름을 nft set 이름으로 사용할 수 있게 정규화
fn sanitize_set_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// 포트 목록 → nft 포트 표현식
/// 단일: `80`  복수: `{ 80, 443, 8000-9000 }`
fn format_port_expr(ports: &[String]) -> String {
    match ports.len() {
        0 => String::new(),
        1 => ports[0].clone(),
        _ => format!("{{ {} }}", ports.join(", ")),
    }
}

/// IP 목록 → nft 인라인 주소 표현식
/// 단일: `1.2.3.4`  복수: `{ 1.2.3.4, 5.6.7.8 }`
fn format_inline_ips(ips: &[String]) -> String {
    match ips.len() {
        0 => String::new(),
        1 => ips[0].clone(),
        _ => format!("{{ {} }}", ips.join(", ")),
    }
}

/// nftables netdev ingress 설정 파일 생성
pub fn generate_nft_policy(policy: &NftPolicy) -> String {
    let mut out = String::new();

    // ── 테이블 헤더 ──────────────────────────────────────────────────────
    out.push_str(&format!("table netdev {} {{\n", policy.table_name));

    // ── 1. 대상 IP set (target_<name>_v4/v6) ────────────────────────────
    out.push_str("    # --- 1. 대상 Pod/컨테이너 IP 그룹 (Targets) ---\n");
    for target in &policy.targets {
        let tname = sanitize_set_name(&target.name);
        if !target.ipv4_addrs.is_empty() {
            out.push_str(&format!(
                "    set target_{}_v4 {{ type ipv4_addr; elements = {{ {} }} }}\n",
                tname,
                target.ipv4_addrs.join(", ")
            ));
        }
        if !target.ipv6_addrs.is_empty() {
            out.push_str(&format!(
                "    set target_{}_v6 {{ type ipv6_addr; elements = {{ {} }} }}\n",
                tname,
                target.ipv6_addrs.join(", ")
            ));
        }
    }

    // ── 2. 서브넷 그룹 set (규칙에서 참조되는 것만) ─────────────────────
    // 참조된 subnet group 이름 수집 (중복 제거)
    let referenced_groups: std::collections::HashSet<String> = policy.targets.iter()
        .flat_map(|t| t.rules.iter())
        .filter_map(|r| r.subnet_group.as_ref())
        .cloned()
        .collect();

    let used_groups: Vec<&NftSubnetGroup> = policy.subnet_groups.iter()
        .filter(|g| referenced_groups.contains(&g.name))
        .collect();

    if !used_groups.is_empty() {
        out.push_str("\n    # --- 2. 서브넷 그룹 (Subnet Groups) ---\n");
        for grp in &used_groups {
            let gname = sanitize_set_name(&grp.name);
            if !grp.cidrs_v4.is_empty() {
                out.push_str(&format!(
                    "    set sg_{}_v4 {{\n        type ipv4_addr; flags interval\n        elements = {{ {} }}\n    }}\n",
                    gname,
                    grp.cidrs_v4.join(", ")
                ));
            }
            if !grp.cidrs_v6.is_empty() {
                out.push_str(&format!(
                    "    set sg_{}_v6 {{\n        type ipv6_addr; flags interval\n        elements = {{ {} }}\n    }}\n",
                    gname,
                    grp.cidrs_v6.join(", ")
                ));
            }
        }
    }

    // ── 3. 전역 규칙 포트 set ────────────────────────────────────────────
    if !policy.global_rules.is_empty() {
        out.push_str("\n    # --- 3. 전역 허용 규칙 포트 집합 ---\n");
        for (idx, grule) in policy.global_rules.iter().enumerate() {
            let range = if grule.port_start == grule.port_end {
                format!("{}", grule.port_start)
            } else {
                format!("{}-{}", grule.port_start, grule.port_end)
            };
            out.push_str(&format!(
                "    set global_{}_{}_ports {{\n        type inet_service; flags interval\n        elements = {{ {} }}\n    }}\n",
                grule.protocol, idx, range
            ));
        }
    } else {
        // 전역 규칙 없으면 traceroute 기본값
        out.push_str("\n    # --- 3. 공통 설정 (Tools) ---\n");
        out.push_str("    set traceroute_udp_ports {\n");
        out.push_str("        type inet_service; flags interval\n");
        out.push_str(&format!(
            "        elements = {{ {}-{} }}\n",
            policy.traceroute_start, policy.traceroute_end
        ));
        out.push_str("    }\n");
    }

    // ── chain ─────────────────────────────────────────────────────────────
    out.push_str(&format!("\n    chain {} {{\n", policy.chain_name));
    out.push_str(&format!(
        "        type filter hook ingress device \"{}\" priority -500; policy accept;\n",
        policy.device_name
    ));

    // 모든 대상 IP 취합 (ICMP/traceroute 인라인 리스트용)
    let all_v4: Vec<String> = policy.targets.iter()
        .flat_map(|t| t.ipv4_addrs.iter().cloned())
        .collect();
    let all_v6: Vec<String> = policy.targets.iter()
        .flat_map(|t| t.ipv6_addrs.iter().cloned())
        .collect();

    // 공통 허용: ICMP + 전역 규칙
    if !all_v4.is_empty() || !all_v6.is_empty() {
        out.push_str("\n        # ==================================================\n");
        out.push_str("        # 공통 허용 규칙 (Global Open: ICMP & 전역 규칙)\n");
        out.push_str("        # ==================================================\n");
    }
    if !all_v4.is_empty() {
        let v4l = format_inline_ips(&all_v4);
        out.push_str(&format!("        ip daddr {} ip protocol icmp accept\n", v4l));
        if !policy.global_rules.is_empty() {
            for (idx, grule) in policy.global_rules.iter().enumerate() {
                out.push_str(&format!(
                    "        ip daddr {} {} dport @global_{}_{}_ports accept\n",
                    v4l, grule.protocol, grule.protocol, idx
                ));
            }
        } else {
            out.push_str(&format!("        ip daddr {} udp dport @traceroute_udp_ports accept\n", v4l));
        }
    }
    if !all_v6.is_empty() {
        let v6l = format_inline_ips(&all_v6);
        out.push_str(&format!("        ip6 daddr {} ip6 nexthdr icmpv6 accept\n", v6l));
        if !policy.global_rules.is_empty() {
            for (idx, grule) in policy.global_rules.iter().enumerate() {
                let proto_kw = if grule.protocol == "tcp" { "tcp" } else { "udp" };
                out.push_str(&format!(
                    "        ip6 daddr {} ip6 nexthdr {} {} dport @global_{}_{}_ports accept\n",
                    v6l, proto_kw, proto_kw, grule.protocol, idx
                ));
            }
        } else {
            out.push_str(&format!("        ip6 daddr {} udp dport @traceroute_udp_ports accept\n", v6l));
        }
    }

    // 서비스별 허용 규칙 (대상별로)
    if !policy.targets.is_empty() {
        out.push_str("\n        # ==================================================\n");
        out.push_str("        # 서비스별 상세 규칙\n");
        out.push_str("        # ==================================================\n");
    }

    // 서비스 맵 (이름 → 정의)
    let svc_map: std::collections::HashMap<&str, &NftServiceDef> = policy.services.iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    for target in &policy.targets {
        let tname = sanitize_set_name(&target.name);
        out.push_str(&format!("\n        # {}\n", target.name.to_uppercase()));

        for rule in &target.rules {
            // 서비스 조회 (없으면 스킵)
            let Some(svc) = svc_map.get(rule.service_name.as_str()) else {
                continue;
            };

            // 소스 필터 결정
            let subnet_group_name = rule.subnet_group.as_deref().filter(|s| !s.is_empty());

            // TCP 규칙
            if !svc.tcp_ports.is_empty() {
                let port_expr = format_port_expr(&svc.tcp_ports);

                if !target.ipv4_addrs.is_empty() {
                    if let Some(gname) = subnet_group_name {
                        // 소스 필터 있는 경우 — 그룹에 v4 CIDR이 있을 때만
                        let grp = policy.subnet_groups.iter().find(|g| g.name == gname);
                        if grp.map(|g| !g.cidrs_v4.is_empty()).unwrap_or(false) {
                            let sname = sanitize_set_name(gname);
                            out.push_str(&format!(
                                "        ip daddr @target_{}_v4 ip saddr @sg_{}_v4 ip protocol tcp tcp dport {} accept\n",
                                tname, sname, port_expr
                            ));
                        }
                    } else {
                        out.push_str(&format!(
                            "        ip daddr @target_{}_v4 ip protocol tcp tcp dport {} accept\n",
                            tname, port_expr
                        ));
                    }
                }

                if !target.ipv6_addrs.is_empty() {
                    if let Some(gname) = subnet_group_name {
                        let grp = policy.subnet_groups.iter().find(|g| g.name == gname);
                        if grp.map(|g| !g.cidrs_v6.is_empty()).unwrap_or(false) {
                            let sname = sanitize_set_name(gname);
                            out.push_str(&format!(
                                "        ip6 daddr @target_{}_v6 ip6 saddr @sg_{}_v6 ip6 nexthdr tcp tcp dport {} accept\n",
                                tname, sname, port_expr
                            ));
                        }
                    } else {
                        out.push_str(&format!(
                            "        ip6 daddr @target_{}_v6 ip6 nexthdr tcp tcp dport {} accept\n",
                            tname, port_expr
                        ));
                    }
                }
            }

            // UDP 규칙
            if !svc.udp_ports.is_empty() {
                let port_expr = format_port_expr(&svc.udp_ports);

                if !target.ipv4_addrs.is_empty() {
                    if let Some(gname) = subnet_group_name {
                        let grp = policy.subnet_groups.iter().find(|g| g.name == gname);
                        if grp.map(|g| !g.cidrs_v4.is_empty()).unwrap_or(false) {
                            let sname = sanitize_set_name(gname);
                            out.push_str(&format!(
                                "        ip daddr @target_{}_v4 ip saddr @sg_{}_v4 ip protocol udp udp dport {} accept\n",
                                tname, sname, port_expr
                            ));
                        }
                    } else {
                        out.push_str(&format!(
                            "        ip daddr @target_{}_v4 ip protocol udp udp dport {} accept\n",
                            tname, port_expr
                        ));
                    }
                }

                if !target.ipv6_addrs.is_empty() {
                    if let Some(gname) = subnet_group_name {
                        let grp = policy.subnet_groups.iter().find(|g| g.name == gname);
                        if grp.map(|g| !g.cidrs_v6.is_empty()).unwrap_or(false) {
                            let sname = sanitize_set_name(gname);
                            out.push_str(&format!(
                                "        ip6 daddr @target_{}_v6 ip6 saddr @sg_{}_v6 ip6 nexthdr udp udp dport {} accept\n",
                                tname, sname, port_expr
                            ));
                        }
                    } else {
                        out.push_str(&format!(
                            "        ip6 daddr @target_{}_v6 ip6 nexthdr udp udp dport {} accept\n",
                            tname, port_expr
                        ));
                    }
                }
            }
        }
    }

    // 대상별 차단 로그 (명시적 drop)
    if !policy.targets.is_empty() {
        out.push_str("\n        # ==================================================\n");
        out.push_str("        # 대상별 개별 차단 로그\n");
        out.push_str("        # ==================================================\n");
        for target in &policy.targets {
            let tname     = sanitize_set_name(&target.name);
            let log_label = target.name.to_uppercase().replace(['-', '.'], "_");
            if !target.ipv4_addrs.is_empty() {
                out.push_str(&format!(
                    "        ip daddr @target_{}_v4 counter log prefix \"NFT_{}_V4_DROP: \" drop\n",
                    tname, log_label
                ));
            }
            if !target.ipv6_addrs.is_empty() {
                out.push_str(&format!(
                    "        ip6 daddr @target_{}_v6 counter log prefix \"NFT_{}_V6_DROP: \" drop\n",
                    tname, log_label
                ));
            }
        }
    }

    out.push_str("\n        accept\n");
    out.push_str("    }\n");
    out.push_str("}\n");

    out
}

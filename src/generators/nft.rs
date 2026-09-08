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

    // established/DNS 응답 우회 (PLAN.md B.1). netdev ingress 훅은 호스트/
    // 컨테이너가 먼저 시작한 아웃바운드 연결의 응답 패킷도 걸러내므로, 이
    // 두 줄이 없으면 컨테이너의 아웃바운드 TCP/DNS 트래픽이 드롭될 수 있다
    // (실제 운영 중인 files/nft/ipvlan_l2.nft과 동일 동작 — 항상 켠다).
    out.push_str("        tcp flags & (ack | rst) != 0 accept\n");
    out.push_str("        udp sport 53 accept\n");

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

/// 클러스터 전 노드에 동일한 `.nft` 파일을 배포하는 Ansible 플레이북 생성
/// (PLAN.md B.2).
///
/// 서비스(pod)는 DRBD/Pacemaker failover로 클러스터의 임의 노드로 옮겨갈 수
/// 있으므로, 목적지 IP 기준 allow 규칙은 pod가 어느 노드에서 뜨든 그 노드의
/// nft 필터에 이미 존재해야 한다 — 실제 운영 중인 `R99.nftables.yml`도
/// 개별 노드가 아니라 클러스터 전체 노드 그룹에 동일 파일을 배포한다.
///
/// 시그니처는 `generate_quadlet_ansible_playbook()`(`src/routes/quadlet.rs`)
/// 및 Pacemaker Ansible 래퍼(`src/routes/pacemaker.rs`)와 동일한 관례를
/// 따른다 — `ansible_hosts`(기본 "all") 자유 입력 문자열 하나로 대상을
/// 정한다 (DRBD처럼 노드별 데이터가 필요 없어 `AnsibleInventory`류의 구조체는
/// 불필요).
pub fn generate_nft_ansible_playbook(
    policy: &NftPolicy,
    ansible_hosts: &str,
    ansible_user: &str,
    ansible_ssh_key: &str,
) -> String {
    // 기존 DB 기본값(DbNftGlobalConfig::nft_file)과 동일한 배포 경로.
    // R99.nftables.yml이 실제로 배포하는 경로와도 일치한다.
    const NFT_DEST_PATH: &str = "/etc/nftables/ipvlan_l2.nft";

    let hosts = if ansible_hosts.trim().is_empty() { "all" } else { ansible_hosts.trim() };
    let user = if ansible_user.trim().is_empty() { "root" } else { ansible_user.trim() };
    let key = if ansible_ssh_key.trim().is_empty() { "~/.ssh/id_rsa" } else { ansible_ssh_key.trim() };

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("- name: nftables netdev ingress 필터 배포 (클러스터 전 노드 동일 적용)\n");
    out.push_str(&format!("  hosts: {}\n", hosts));
    out.push_str(&format!("  remote_user: {}\n", user));
    out.push_str("  become: yes\n");
    out.push_str("  vars:\n");
    out.push_str(&format!("    ansible_ssh_private_key_file: {}\n", key));
    out.push_str("  tasks:\n");

    out.push_str(&format!("    - name: {} 배포\n", NFT_DEST_PATH));
    out.push_str("      ansible.builtin.copy:\n");
    out.push_str(&format!("        dest: {}\n", NFT_DEST_PATH));
    out.push_str("        backup: yes\n");
    out.push_str("        mode: '0600'\n");
    out.push_str("        content: |\n");
    for line in generate_nft_policy(policy).lines() {
        out.push_str(&format!("          {}\n", line));
    }
    out.push_str("      notify: restart nftables service\n\n");

    out.push_str("    - name: /etc/sysconfig/nftables.conf에 include 보장\n");
    out.push_str("      ansible.builtin.lineinfile:\n");
    out.push_str("        path: /etc/sysconfig/nftables.conf\n");
    out.push_str(&format!("        line: 'include \"{}\"'\n", NFT_DEST_PATH));
    out.push_str("        state: present\n");
    out.push_str("        backup: yes\n");
    out.push_str("        owner: root\n");
    out.push_str("        group: root\n");
    out.push_str("        mode: '0600'\n");
    out.push_str("      notify: restart nftables service\n\n");

    out.push_str("  handlers:\n");
    out.push_str("    - name: restart nftables service\n");
    out.push_str("      ansible.builtin.systemd:\n");
    out.push_str("        name: nftables\n");
    out.push_str("        state: restarted\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_policy() -> NftPolicy {
        NftPolicy {
            filename: "filter_ingress.nft".to_string(),
            table_name: "filter_ingress".to_string(),
            device_name: "eth0".to_string(),
            chain_name: "ingress_eth0".to_string(),
            traceroute_start: 33434,
            traceroute_end: 65535,
            global_rules: Vec::new(),
            targets: Vec::new(),
            subnet_groups: Vec::new(),
            services: Vec::new(),
        }
    }

    #[test]
    fn established_and_dns_bypass_immediately_follow_chain_header() {
        let out = generate_nft_policy(&sample_policy());
        let header_pos = out
            .find("type filter hook ingress")
            .expect("chain header missing");
        let ack_rst_pos = out
            .find("tcp flags & (ack | rst) != 0 accept")
            .expect("established bypass missing");
        let dns_pos = out
            .find("udp sport 53 accept")
            .expect("dns bypass missing");
        assert!(ack_rst_pos > header_pos);
        assert!(dns_pos > ack_rst_pos);
    }

    #[test]
    fn ansible_playbook_uses_given_hosts_user_key_and_r99_paths() {
        let playbook = generate_nft_ansible_playbook(
            &sample_policy(),
            "container_service",
            "deploy",
            "~/.ssh/deploy_key",
        );
        assert!(playbook.contains("hosts: container_service"));
        assert!(playbook.contains("remote_user: deploy"));
        assert!(playbook.contains("ansible_ssh_private_key_file: ~/.ssh/deploy_key"));
        assert!(playbook.contains("dest: /etc/nftables/ipvlan_l2.nft"));
        assert!(playbook.contains("include \"/etc/nftables/ipvlan_l2.nft\""));
        assert!(playbook.contains("backup: yes"));
        assert!(playbook.contains("notify: restart nftables service"));
        assert!(playbook.contains("name: restart nftables service"));
    }

    #[test]
    fn ansible_playbook_falls_back_to_defaults_when_inputs_empty() {
        let playbook = generate_nft_ansible_playbook(&sample_policy(), "", "", "");
        assert!(playbook.contains("hosts: all"));
        assert!(playbook.contains("remote_user: root"));
        assert!(playbook.contains("ansible_ssh_private_key_file: ~/.ssh/id_rsa"));
    }
}

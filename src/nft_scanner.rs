//! nft 파일 스캐너: 파일 존재 보장 + 파싱 + DB 업데이트

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use rusqlite::Connection;

use crate::db::{self, DbAnsibleProfile, DbNftGlobalConfig, DbNftTarget, DbNftTargetRule};
use crate::models::nft::{NftSubnetGroup, NftServiceDef};

pub struct NftScanResult {
    pub file_existed:     bool,
    pub groups_updated:   usize,
    pub services_updated: usize,
    pub targets_updated:  usize,
    pub ansible_output:   Option<String>,
    pub message:          String,
}

/// nft 파일이 없으면 Ansible로 생성하고, 파일을 파싱해서 DB에 저장합니다.
/// profile 이 None 이면 localhost 로컬 실행, Some 이면 DB 노드 목록에 대해 원격 실행합니다.
pub async fn ensure_and_scan(
    temp_dir: &str,
    nft_file: &str,
    db: &Arc<Mutex<Connection>>,
    profile: Option<&DbAnsibleProfile>,
) -> anyhow::Result<NftScanResult> {
    let file_existed = std::path::Path::new(nft_file).exists();
    let ansible_output: Option<String>;

    if !file_existed {
        if let Err(e) = std::fs::create_dir_all(temp_dir) {
            return Err(anyhow::anyhow!("temp_dir 생성 실패: {}", e));
        }

        let (hosts_block, become_val) = if let Some(p) = profile {
            let nodes = {
                let conn = db.lock().unwrap();
                db::list_nodes(&conn).unwrap_or_default()
            };
            let become_val = if p.do_become { "true" } else { "false" };
            let hosts_block = if nodes.is_empty() {
                "  hosts: localhost\n  connection: local\n".to_string()
            } else {
                "  hosts: all\n".to_string()
            };
            (hosts_block, become_val.to_string())
        } else {
            ("  hosts: localhost\n  connection: local\n".to_string(), "true".to_string())
        };

        let playbook_path = format!("{}/setup_nftables.yml", temp_dir);
        let playbook_content = format!(
r#"---
- name: nftables ipvlan_l2 설정 초기화
{hosts_block}  become: {become_val}
  tasks:
    - name: /etc/nftables 디렉토리 생성
      file:
        path: /etc/nftables
        state: directory
        mode: '0750'

    - name: ipvlan_l2.nft 파일 생성 (없으면)
      copy:
        dest: "{nft_file}"
        content: |
          # nftables ipvlan L2 방화벽 규칙
          # HA Container Manager에서 자동 관리됩니다
        mode: '0640'
        force: no

    - name: /etc/sysconfig/nftables.conf에 include 추가
      lineinfile:
        path: /etc/sysconfig/nftables.conf
        line: 'include "{nft_file}"'
        create: yes
        state: present
"#,
            hosts_block = hosts_block,
            become_val = become_val,
            nft_file = nft_file,
        );

        std::fs::write(&playbook_path, &playbook_content)?;

        // 인벤토리 파일 작성 (프로파일 사용 시)
        let ansible_args: Vec<String> = if let Some(p) = profile {
            let nodes = {
                let conn = db.lock().unwrap();
                db::list_nodes(&conn).unwrap_or_default()
            };
            if !nodes.is_empty() {
                let inventory_path = format!("{}/nft_inventory.yml", temp_dir);
                let mut inv_lines = vec!["all:".to_string(), "  hosts:".to_string()];
                for node in &nodes {
                    inv_lines.push(format!("    {}:", node.hostname));
                    inv_lines.push(format!("      ansible_host: {}", node.ip));
                }
                inv_lines.push("  vars:".to_string());
                inv_lines.push(format!("    ansible_user: {}", p.ssh_user));
                if p.auth_method == "key" && !p.ssh_key.is_empty() {
                    inv_lines.push(format!("    ansible_ssh_private_key_file: {}", p.ssh_key));
                } else if !p.ssh_password.is_empty() {
                    inv_lines.push(format!("    ansible_ssh_pass: {}", p.ssh_password));
                }
                if p.do_become {
                    inv_lines.push("    ansible_become: true".to_string());
                    if !p.become_method.is_empty() {
                        inv_lines.push(format!("    ansible_become_method: {}", p.become_method));
                    }
                    if !p.become_password.is_empty() {
                        inv_lines.push(format!("    ansible_become_password: {}", p.become_password));
                    }
                }
                std::fs::write(&inventory_path, inv_lines.join("\n") + "\n")?;
                vec!["-i".to_string(), inventory_path, playbook_path]
            } else {
                vec!["-c".to_string(), "local".to_string(), "-i".to_string(), "localhost,".to_string(), playbook_path]
            }
        } else {
            vec!["-c".to_string(), "local".to_string(), "-i".to_string(), "localhost,".to_string(), playbook_path]
        };

        let output = tokio::process::Command::new("ansible-playbook")
            .args(&ansible_args)
            .output()
            .await;

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let combined = if stderr.is_empty() {
                    stdout
                } else {
                    format!("{}\nSTDERR:\n{}", stdout, stderr)
                };
                ansible_output = Some(combined);
                if !o.status.success() {
                    return Ok(NftScanResult {
                        file_existed: false,
                        groups_updated: 0,
                        services_updated: 0,
                        targets_updated: 0,
                        ansible_output,
                        message: "Ansible 실행 실패 - 파일이 생성되지 않았을 수 있습니다".to_string(),
                    });
                }
            }
            Err(e) => {
                ansible_output = Some(format!("ansible-playbook 실행 오류: {}", e));
                return Ok(NftScanResult {
                    file_existed: false,
                    groups_updated: 0,
                    services_updated: 0,
                    targets_updated: 0,
                    ansible_output,
                    message: format!("Ansible 실행 실패: {}", e),
                });
            }
        }
    } else {
        ansible_output = None;
    }

    // 파일 읽기
    let content = match std::fs::read_to_string(nft_file) {
        Ok(c) => c,
        Err(e) => {
            return Ok(NftScanResult {
                file_existed,
                groups_updated: 0,
                services_updated: 0,
                targets_updated: 0,
                ansible_output,
                message: format!("파일 읽기 실패 {}: {}", nft_file, e),
            });
        }
    };

    // 파싱
    let parsed = parse_nft_file(&content);

    // DB 업데이트
    let mut groups_updated = 0usize;
    let mut services_updated = 0usize;
    let mut targets_updated = 0usize;

    // 1. global config 업데이트
    {
        let conn = db.lock().unwrap();
        let current = db::get_nft_global_config(&conn).unwrap_or(DbNftGlobalConfig {
            table_name:       "filter_ingress".to_string(),
            device_name:      "eth0".to_string(),
            chain_name:       String::new(),
            traceroute_start: 33434,
            traceroute_end:   65535,
            nft_file:         nft_file.to_string(),
            updated_at:       String::new(),
        });
        let updated_cfg = DbNftGlobalConfig {
            table_name:       parsed.table_name.clone().unwrap_or(current.table_name),
            device_name:      parsed.device_name.clone().unwrap_or(current.device_name),
            chain_name:       parsed.chain_name.clone().unwrap_or(current.chain_name),
            traceroute_start: parsed.traceroute_start.unwrap_or(current.traceroute_start),
            traceroute_end:   parsed.traceroute_end.unwrap_or(current.traceroute_end),
            nft_file:         nft_file.to_string(),
            updated_at:       String::new(),
        };
        let _ = db::update_nft_global_config(&conn, &updated_cfg);
    }

    // 2. subnet groups 업데이트
    {
        let conn = db.lock().unwrap();
        for (name, (cidrs_v4, cidrs_v6)) in &parsed.subnet_groups {
            let grp = NftSubnetGroup {
                id:          0,
                name:        name.clone(),
                description: "자동 감지".to_string(),
                cidrs_v4:    cidrs_v4.clone(),
                cidrs_v6:    cidrs_v6.clone(),
            };
            if db::upsert_nft_subnet_group(&conn, &grp).is_ok() {
                groups_updated += 1;
            }
        }
    }

    // 3. auto-services 업데이트 (이름 충돌 시 무시)
    {
        let conn = db.lock().unwrap();
        for svc in &parsed.auto_services {
            if db::insert_nft_service_if_not_exists(&conn, svc).is_ok() {
                services_updated += 1;
            }
        }
    }

    // 4. targets 업데이트
    {
        let conn = db.lock().unwrap();
        for target in &parsed.targets {
            if db::upsert_nft_target(&conn, target).is_ok() {
                targets_updated += 1;
            }
        }
    }

    let msg = format!(
        "스캔 완료 (파일 {}): 그룹 {}개, 서비스 {}개, 대상 {}개 업데이트",
        if file_existed { "기존" } else { "신규 생성" },
        groups_updated, services_updated, targets_updated,
    );

    Ok(NftScanResult {
        file_existed,
        groups_updated,
        services_updated,
        targets_updated,
        ansible_output,
        message: msg,
    })
}

// ─── 파서 내부 구조 ─────────────────────────────────────────────────────────

struct ParsedNft {
    table_name:       Option<String>,
    device_name:      Option<String>,
    chain_name:       Option<String>,
    traceroute_start: Option<u16>,
    traceroute_end:   Option<u16>,
    /// name → (cidrs_v4, cidrs_v6)
    subnet_groups:    HashMap<String, (Vec<String>, Vec<String>)>,
    auto_services:    Vec<NftServiceDef>,
    targets:          Vec<DbNftTarget>,
}

/// nft 파일 내용을 파싱합니다.
fn parse_nft_file(content: &str) -> ParsedNft {
    let mut result = ParsedNft {
        table_name:       None,
        device_name:      None,
        chain_name:       None,
        traceroute_start: None,
        traceroute_end:   None,
        subnet_groups:    HashMap::new(),
        auto_services:    Vec::new(),
        targets:          Vec::new(), // will be populated after parsing
    };

    // target_name → (ipv4_addrs, ipv6_addrs)
    let mut target_addrs: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    // target_name → rules
    let mut target_rules: HashMap<String, Vec<DbNftTargetRule>> = HashMap::new();
    // (protocol, sorted_ports_key) → service_name
    let mut service_map: HashMap<String, String> = HashMap::new();

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    let mut in_table = false;
    let mut table_depth: i32 = 0;
    let mut current_set_name = String::new();
    let mut in_set = false;
    let mut in_chain = false;
    // element accumulator (multi-line)
    let mut elem_buf = String::new();
    let mut in_elements = false;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // 주석 건너뜀
        if trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        // table netdev X {
        if !in_table {
            if trimmed.starts_with("table ") && trimmed.contains('{') {
                // table netdev <name> {
                let parts: Vec<&str> = trimmed.splitn(4, ' ').collect();
                if parts.len() >= 3 {
                    result.table_name = Some(parts[2].trim_end_matches('{').trim().to_string());
                }
                in_table = true;
                table_depth = 1;
                i += 1;
                continue;
            }
        }

        if !in_table {
            i += 1;
            continue;
        }

        // 중괄호 깊이 업데이트 (table 레벨)
        let open_count = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let close_count = trimmed.chars().filter(|&c| c == '}').count() as i32;

        // --- set 파싱 ---
        if !in_set && !in_chain && trimmed.starts_with("set ") && trimmed.ends_with('{') {
            let set_name = trimmed
                .trim_start_matches("set ")
                .trim_end_matches('{')
                .trim()
                .to_string();
            current_set_name = set_name;
            in_set = true;
            i += 1;
            continue;
        }

        if in_set {
            // elements = { ... } (possibly multi-line)
            if trimmed.starts_with("elements = {") || in_elements {
                if !in_elements {
                    // 시작
                    elem_buf = trimmed.to_string();
                    if elem_buf.contains('}') {
                        // 한 줄에 완성됨
                        process_set_elements(&current_set_name, &elem_buf, &mut result, &mut target_addrs);
                    } else {
                        in_elements = true;
                    }
                } else {
                    // 계속 누적
                    elem_buf.push(' ');
                    elem_buf.push_str(trimmed);
                    if trimmed.contains('}') {
                        in_elements = false;
                        process_set_elements(&current_set_name, &elem_buf, &mut result, &mut target_addrs);
                        elem_buf.clear();
                    }
                }
            }

            // traceroute_udp_ports set 처리
            if current_set_name == "traceroute_udp_ports" {
                if let Some(range_str) = extract_elements_content(trimmed) {
                    if let Some((start, end)) = parse_port_range(&range_str) {
                        result.traceroute_start = Some(start);
                        result.traceroute_end   = Some(end);
                    }
                }
            }

            // set 닫힘 감지
            if trimmed == "}" {
                in_set = false;
                current_set_name.clear();
            }
            i += 1;
            continue;
        }

        // --- chain 파싱 ---
        if !in_chain && trimmed.starts_with("chain ") && trimmed.ends_with('{') {
            let chain_name = trimmed
                .trim_start_matches("chain ")
                .trim_end_matches('{')
                .trim()
                .to_string();
            result.chain_name = Some(chain_name);
            in_chain = true;
            i += 1;
            continue;
        }

        if in_chain {
            // hook ingress device "X" → device_name
            if trimmed.contains("hook ingress device") {
                if let Some(dev) = extract_device_name(trimmed) {
                    result.device_name = Some(dev);
                }
            }

            // IPv4 accept 규칙 파싱 (IPv6 건너뜀)
            if trimmed.starts_with("ip daddr @target_") && trimmed.ends_with("accept") {
                parse_chain_rule(trimmed, &mut target_rules, &mut service_map, &mut result.auto_services);
            }

            // chain 닫힘
            if trimmed == "}" {
                in_chain = false;
            }
            i += 1;
            continue;
        }

        // table 닫힘
        if trimmed == "}" && table_depth == 1 {
            in_table = false;
            table_depth = 0;
        } else {
            // 깊이 업데이트 (set/chain 내부가 아닐 때)
            table_depth += open_count - close_count;
        }
        i += 1;
    }

    // 수집된 target_addrs + target_rules → DbNftTarget 목록 생성
    let mut all_names: Vec<String> = {
        let mut names: Vec<String> = target_addrs.keys().cloned().collect();
        for k in target_rules.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
        names.sort();
        names
    };

    result.targets = all_names.drain(..).map(|name| {
        let (ipv4_addrs, ipv6_addrs) = target_addrs.remove(&name).unwrap_or_default();
        let rules = target_rules.remove(&name).unwrap_or_default();
        DbNftTarget {
            id: 0,
            name,
            ipv4_addrs,
            ipv6_addrs,
            rules,
        }
    }).collect();

    result
}

/// `set target_X_v4 { elements = { ... } }` 등을 처리
fn process_set_elements(
    set_name: &str,
    line: &str,
    result: &mut ParsedNft,
    target_addrs: &mut HashMap<String, (Vec<String>, Vec<String>)>,
) {
    let elems = match extract_elements_content(line) {
        Some(e) => e,
        None    => return,
    };

    if set_name.starts_with("target_") {
        // target_X_v4 or target_X_v6
        if let Some(rest) = set_name.strip_prefix("target_") {
            if let Some(tname) = rest.strip_suffix("_v4") {
                let addrs = parse_comma_list(&elems);
                let entry = target_addrs.entry(tname.to_string()).or_default();
                entry.0 = addrs;
            } else if let Some(tname) = rest.strip_suffix("_v6") {
                let addrs = parse_comma_list(&elems);
                let entry = target_addrs.entry(tname.to_string()).or_default();
                entry.1 = addrs;
            }
        }
    } else if set_name.starts_with("sg_") {
        // sg_X_v4 or sg_X_v6
        if let Some(rest) = set_name.strip_prefix("sg_") {
            if let Some(gname) = rest.strip_suffix("_v4") {
                let cidrs = parse_comma_list(&elems);
                let entry = result.subnet_groups.entry(gname.to_string()).or_default();
                entry.0 = cidrs;
            } else if let Some(gname) = rest.strip_suffix("_v6") {
                let cidrs = parse_comma_list(&elems);
                let entry = result.subnet_groups.entry(gname.to_string()).or_default();
                entry.1 = cidrs;
            }
        }
    }
    // traceroute_udp_ports 는 parse_nft_file 에서 별도 처리
}

/// `elements = { ... }` 또는 `{ ... }` 에서 내용 추출
fn extract_elements_content(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start { return None; }
    Some(s[start + 1..end].to_string())
}

/// "a, b, c" → vec!["a", "b", "c"]
fn parse_comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// "33434-65535" → Some((33434, 65535))
fn parse_port_range(s: &str) -> Option<(u16, u16)> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once('-') {
        let start = a.trim().parse::<u16>().ok()?;
        let end   = b.trim().parse::<u16>().ok()?;
        return Some((start, end));
    }
    None
}

/// `hook ingress device "X"` 에서 device 이름 추출
fn extract_device_name(s: &str) -> Option<String> {
    let marker = "device \"";
    let start  = s.find(marker)? + marker.len();
    let end    = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

/// 포트 표현식 파싱: "22" | "{ 80, 443 }" | "{ 80, 443, 8000-9000 }"
fn parse_port_expr(expr: &str) -> Vec<String> {
    let inner = if expr.contains('{') {
        expr.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string()
    } else {
        expr.trim().to_string()
    };
    inner
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// chain 내부 IPv4 accept 규칙 파싱
/// 형식:
///   ip daddr @target_X_v4 ip protocol tcp tcp dport PORTEXPR accept
///   ip daddr @target_X_v4 ip saddr @sg_Y_v4 ip protocol tcp tcp dport PORTEXPR accept
fn parse_chain_rule(
    line: &str,
    target_rules: &mut HashMap<String, Vec<DbNftTargetRule>>,
    service_map:  &mut HashMap<String, String>,
    auto_services: &mut Vec<NftServiceDef>,
) {
    // target 이름 추출
    let target_name = {
        let marker = "@target_";
        let start = match line.find(marker) {
            Some(s) => s + marker.len(),
            None    => return,
        };
        let rest = &line[start..];
        // 끝: _v4
        let end = match rest.find("_v4") {
            Some(e) => e,
            None    => return,
        };
        rest[..end].to_string()
    };

    // subnet_group 이름 (있으면)
    let subnet_group_name: String = if let Some(pos) = line.find("@sg_") {
        let start = pos + 4; // "@sg_" 다음
        let rest  = &line[start..];
        let end   = rest.find("_v4").or_else(|| rest.find(' ')).unwrap_or(rest.len());
        rest[..end].to_string()
    } else {
        String::new()
    };

    // protocol
    let protocol = if line.contains("ip protocol tcp") || line.contains("ip6 nexthdr tcp") {
        "tcp"
    } else if line.contains("ip protocol udp") || line.contains("ip6 nexthdr udp") {
        "udp"
    } else {
        return; // ICMP 등 건너뜀
    };

    // dport 표현식 추출
    let dport_marker = if protocol == "tcp" { "tcp dport " } else { "udp dport " };
    let port_expr = {
        let pos = match line.find(dport_marker) {
            Some(p) => p + dport_marker.len(),
            None    => return,
        };
        let rest = &line[pos..];
        // { ... } 또는 단일 포트
        if rest.starts_with('{') {
            let end = rest.find('}').map(|e| e + 1).unwrap_or(rest.len());
            rest[..end].to_string()
        } else {
            // 단일 포트: 다음 공백까지
            rest.split_whitespace().next().unwrap_or("").to_string()
        }
    };

    let mut ports = parse_port_expr(&port_expr);
    ports.sort();

    // service key
    let service_key = format!("{}_{}_{}", protocol, ports.join("_"), subnet_group_name);

    let service_name = if let Some(sn) = service_map.get(&service_key) {
        sn.clone()
    } else {
        let sn = format!("auto_{}_{}", protocol, ports.join("_"));
        // auto_service 생성
        let svc = NftServiceDef {
            id:          0,
            name:        sn.clone(),
            description: "자동 감지".to_string(),
            tcp_ports:   if protocol == "tcp" { ports.clone() } else { Vec::new() },
            udp_ports:   if protocol == "udp" { ports.clone() } else { Vec::new() },
        };
        // 중복 방지
        if !auto_services.iter().any(|s| s.name == sn) {
            auto_services.push(svc);
        }
        service_map.insert(service_key, sn.clone());
        sn
    };

    let rules = target_rules.entry(target_name).or_default();
    // 중복 규칙 방지
    let already = rules.iter().any(|r| {
        r.service_name == service_name && r.subnet_group_name == subnet_group_name
    });
    if !already {
        rules.push(DbNftTargetRule {
            id:                0,
            service_name,
            subnet_group_name,
        });
    }
}

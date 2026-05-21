use crate::models::quadlet::{QuadletContainer, QuadletNetwork, QuadletPod, QuadletVolume, QuadletConfig, PodNetworkEntry};

/// .volume 유닛 파일 생성
pub fn generate_volume_unit(vol: &QuadletVolume) -> String {
    let mut out = String::new();

    // [Unit] Description (NFS 등 특수 볼륨)
    let has_type = vol.options.iter().any(|o| o.to_lowercase().starts_with("type="));
    if has_type {
        out.push_str("[Unit]\n");
        out.push_str(&format!("Description=Volume {}\n\n", vol.name));
    }

    out.push_str("[Volume]\n");
    out.push_str(&format!("VolumeName={}\n", vol.name));
    if !vol.driver.is_empty() {
        out.push_str(&format!("Driver={}\n", vol.driver));
    }
    for opt in &vol.options {
        // type=nfs, device=..., o=... 등은 별도 키로 분리
        if let Some(val) = opt.strip_prefix("type=") {
            out.push_str(&format!("Type={}\n", val));
        } else if let Some(val) = opt.strip_prefix("device=") {
            out.push_str(&format!("Device={}\n", val));
        } else {
            out.push_str(&format!("Options={}\n", opt));
        }
    }
    for label in &vol.labels {
        out.push_str(&format!("Label={}\n", label));
    }
    out.push('\n');
    out.push_str("[Install]\n");
    out.push_str("WantedBy=multi-user.target\n");
    out
}

/// .network 유닛 파일 생성 (IPVLAN / macvlan)
pub fn generate_network_unit(net: &QuadletNetwork) -> String {
    let mut out = String::new();
    out.push_str("[Network]\n");
    out.push_str(&format!("Driver={}\n", net.driver));

    // IPv6= 라인: 듀얼스택이면 true, IPv6 없는 ipvlan/macvlan이면 no
    let is_dual = !net.subnet6.is_empty();
    if net.driver == "ipvlan" || net.driver == "macvlan" {
        if is_dual || net.ipv6 {
            out.push_str("IPv6=true\n");
        } else {
            out.push_str("IPv6=no\n");
        }
    }

    // IPv4
    if !net.subnet.is_empty() {
        out.push_str(&format!("Subnet={}\n", net.subnet));
    }
    if !net.gateway.is_empty() {
        out.push_str(&format!("Gateway={}\n", net.gateway));
    }

    // IPv6 (듀얼스택)
    if !net.subnet6.is_empty() {
        out.push_str(&format!("Subnet={}\n", net.subnet6));
    }
    if !net.gateway6.is_empty() {
        out.push_str(&format!("Gateway={}\n", net.gateway6));
    }

    // 부모 인터페이스 + mode를 한 줄로 결합
    let mode_part = if !net.ipvlan_mode.is_empty() {
        format!(",mode={}", net.ipvlan_mode)
    } else {
        String::new()
    };

    if !net.interface.is_empty() {
        out.push_str(&format!("Options=parent={}{}\n", net.interface, mode_part));
    } else if net.driver == "ipvlan" || net.driver == "macvlan" {
        // 인터페이스 미지정 시 Ansible 배포 시 자동감지 placeholder
        out.push_str(&format!("Options=parent=__PARENT_IFACE__{}\n", mode_part));
    }

    // Internal 네트워크 (외부 접근 차단)
    if net.internal {
        out.push_str("Internal=true\n");
    }

    for opt in &net.options {
        out.push_str(&format!("Options={}\n", opt));
    }
    out
}

/// .pod 유닛 파일 생성
pub fn generate_pod_unit(pod: &QuadletPod) -> String {
    let mut out = String::new();

    // [Unit] 섹션: 네트워크 의존성
    let valid_nets: Vec<&PodNetworkEntry> = pod.networks.iter()
        .filter(|e| !e.network.is_empty())
        .collect();

    let has_unit = !valid_nets.is_empty() || pod.description.as_deref().map_or(false, |s| !s.is_empty());
    if has_unit {
        out.push_str("[Unit]\n");
        if let Some(desc) = &pod.description {
            if !desc.is_empty() {
                out.push_str(&format!("Description={}\n", desc));
            }
        }
        if !valid_nets.is_empty() {
            // After=net1.network net2.network (한 줄로 병합)
            let after_list: Vec<String> = valid_nets.iter()
                .map(|e| format!("{}.network", e.network))
                .collect();
            out.push_str(&format!("After={}\n", after_list.join(" ")));
            out.push_str(&format!("Requires={}\n", after_list.join(" ")));
        }
        out.push('\n');
    }

    out.push_str("[Pod]\n");
    out.push_str(&format!("PodName={}\n", pod.name));

    if let Some(hostname) = &pod.hostname {
        if !hostname.is_empty() {
            out.push_str(&format!("HostName={}\n", hostname));
        }
    }
    if let Some(shm) = &pod.shm_size {
        if !shm.is_empty() {
            out.push_str(&format!("ShmSize={}\n", shm));
        }
    }

    for entry in &valid_nets {
        out.push_str(&format!("Network={}", build_network_param(entry)));
        out.push('\n');
    }

    for label in &pod.labels {
        out.push_str(&format!("Label={}\n", label));
    }
    out
}

/// Pod Network= 파라미터 문자열 빌드 (쉼표 구분)
/// 형식: netname.network:ip=X,ip6=Y,gateway=G,gateway6=G6
fn build_network_param(entry: &PodNetworkEntry) -> String {
    let mut opts: Vec<String> = Vec::new();

    if let Some(ip) = &entry.ip {
        if !ip.is_empty() { opts.push(format!("ip={}", ip)); }
    }
    if let Some(ip6) = &entry.ip6 {
        if !ip6.is_empty() { opts.push(format!("ip6={}", ip6)); }
    }
    if let Some(gw) = &entry.gateway {
        if !gw.is_empty() { opts.push(format!("gateway={}", gw)); }
    }
    if let Some(gw6) = &entry.gateway6 {
        if !gw6.is_empty() { opts.push(format!("gateway6={}", gw6)); }
    }

    if opts.is_empty() {
        format!("{}.network", entry.network)
    } else {
        format!("{}.network:{}", entry.network, opts.join(","))
    }
}

/// DRBD/호스트 바인드 마운트용 .volume 유닛 파일 생성
pub fn generate_bind_volume_unit(name: &str, host_path: &str, description: &str) -> String {
    let mut out = String::new();
    let desc = if description.is_empty() { name } else { description };
    out.push_str("[Unit]\n");
    out.push_str(&format!("Description={}\n\n", desc));
    out.push_str("[Volume]\n");
    out.push_str(&format!("VolumeName={}\n", name));
    out.push_str("Driver=local\n");
    if !host_path.is_empty() {
        out.push_str(&format!("Device={}\n", host_path));
        out.push_str("Options=bind\n");
    }
    out.push('\n');
    out.push_str("[Install]\n");
    out.push_str("WantedBy=multi-user.target\n");
    out
}

/// .container 유닛 파일 생성
pub fn generate_container_unit(c: &QuadletContainer) -> String {
    let mut out = String::new();

    // [Unit] 섹션: Description + 의존성
    let has_desc = c.description.as_deref().map_or(false, |s| !s.is_empty());
    let has_deps = !c.depends_on.is_empty();
    if has_desc || has_deps {
        out.push_str("[Unit]\n");
        if let Some(desc) = &c.description {
            if !desc.is_empty() {
                out.push_str(&format!("Description={}\n", desc));
            }
        }
        if has_deps {
            // After= / Requires= 한 줄로 병합
            let after_parts: Vec<String> = c.depends_on.iter().map(|dep| {
                let suffix = if c.pod.as_deref() == Some(dep.as_str()) { "pod" } else { "service" };
                format!("{}.{}", dep, suffix)
            }).collect();
            out.push_str(&format!("After={}\n", after_parts.join(" ")));
            out.push_str(&format!("Requires={}\n", after_parts.join(" ")));
        }
        out.push('\n');
    }

    out.push_str("[Container]\n");
    out.push_str(&format!("ContainerName={}\n", c.name));
    out.push_str(&format!("Image={}\n", c.image));

    // pod 소속
    if let Some(pod) = &c.pod {
        if !pod.is_empty() {
            out.push_str(&format!("Pod={}.pod\n", pod));
        }
    } else {
        // pod 없을 때만 network/ip 직접 지정
        if let Some(net) = &c.network {
            if !net.is_empty() {
                out.push_str(&format!("Network={}.network\n", net));
            }
        }
        if let Some(ip) = &c.ip {
            if !ip.is_empty() {
                out.push_str(&format!("IP={}\n", ip));
            }
        }
        for port in &c.publish_ports {
            out.push_str(&format!("PublishPort={}\n", port));
        }
    }

    // 환경변수
    for (k, v) in &c.environment {
        out.push_str(&format!("Environment={}={}\n", k, v));
    }

    // 볼륨
    for vol in &c.volumes {
        out.push_str(&format!("Volume={}\n", vol));
    }

    // tmpfs
    for t in &c.tmpfs {
        out.push_str(&format!("Tmpfs={}\n", t));
    }

    // exec
    if let Some(exec) = &c.exec {
        if !exec.is_empty() {
            out.push_str(&format!("Exec={}\n", exec));
        }
    }

    // entrypoint
    if let Some(ep) = &c.entrypoint {
        if !ep.is_empty() {
            out.push_str(&format!("Entrypoint={}\n", ep));
        }
    }

    // user
    if let Some(user) = &c.user {
        if !user.is_empty() {
            out.push_str(&format!("User={}\n", user));
        }
    }

    // auto_update
    if let Some(au) = &c.auto_update {
        if !au.is_empty() {
            out.push_str(&format!("AutoUpdate={}\n", au));
        }
    }

    // 라벨
    for label in &c.labels {
        out.push_str(&format!("Label={}\n", label));
    }

    // 추가 podman 인수
    for arg in &c.extra_args {
        out.push_str(&format!("PodmanArgs={}\n", arg));
    }

    out.push('\n');
    out.push_str("[Service]\n");

    // service type
    if let Some(stype) = &c.service_type {
        if !stype.is_empty() {
            out.push_str(&format!("Type={}\n", stype));
        }
    }

    out.push_str("Restart=no\n");
    out.push_str("TimeoutStartSec=0\n");

    if let Some(stop) = &c.timeout_stop_sec {
        if !stop.is_empty() {
            out.push_str(&format!("TimeoutStopSec={}\n", stop));
        }
    }

    if let Some(rae) = c.remain_after_exit {
        out.push_str(&format!("RemainAfterExit={}\n", if rae { "yes" } else { "no" }));
    }

    out
}

/// 모든 Quadlet 파일을 (파일명, 내용) 쌍으로 반환
pub fn generate_all_units(config: &QuadletConfig) -> Vec<(String, String)> {
    let mut files = Vec::new();

    for vol in &config.volumes {
        let filename = format!("{}.volume", vol.name);
        files.push((filename, generate_volume_unit(vol)));
    }
    for net in &config.networks {
        let filename = format!("{}.network", net.name);
        files.push((filename, generate_network_unit(net)));
    }
    for pod in &config.pods {
        let filename = format!("{}.pod", pod.name);
        files.push((filename, generate_pod_unit(pod)));
    }
    for container in &config.containers {
        let filename = format!("{}.container", container.name);
        files.push((filename, generate_container_unit(container)));
    }

    files
}

/// podman 컨테이너 정보에서 QuadletContainer 자동 생성
pub fn podman_inspect_to_quadlet(inspect_json: &str) -> anyhow::Result<Vec<QuadletContainer>> {
    let data: serde_json::Value = serde_json::from_str(inspect_json)?;
    let arr = data.as_array().ok_or_else(|| anyhow::anyhow!("JSON 배열이 아닙니다"))?;

    let mut containers = Vec::new();
    for item in arr {
        let name = item["Name"]
            .as_str()
            .unwrap_or("unknown")
            .trim_start_matches('/')
            .to_string();
        let image = item["ImageName"]
            .as_str()
            .or_else(|| item["Config"]["Image"].as_str())
            .unwrap_or("")
            .to_string();

        // 환경변수
        let env: Vec<(String, String)> = item["Config"]["Env"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|e| {
                let s = e.as_str()?;
                let mut parts = s.splitn(2, '=');
                let k = parts.next()?.to_string();
                let v = parts.next().unwrap_or("").to_string();
                Some((k, v))
            })
            .collect();

        // 볼륨 마운트
        let volumes: Vec<String> = item["Mounts"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                let src = m["Source"].as_str()?;
                let dst = m["Destination"].as_str()?;
                let mode = m["Mode"].as_str().unwrap_or("rw");
                Some(format!("{}:{}:{}", src, dst, mode))
            })
            .collect();

        // 포트
        let publish_ports: Vec<String> = item["NetworkSettings"]["Ports"]
            .as_object()
            .map(|ports| {
                ports.iter().filter_map(|(container_port, host_bindings)| {
                    let bindings = host_bindings.as_array()?;
                    bindings.first().and_then(|b| {
                        let host_port = b["HostPort"].as_str()?;
                        Some(format!("{}:{}", host_port, container_port))
                    })
                }).collect()
            })
            .unwrap_or_default();

        // 네트워크
        let network = item["NetworkSettings"]["Networks"]
            .as_object()
            .and_then(|nets| nets.keys().next().cloned());

        // IP
        let ip = item["NetworkSettings"]["Networks"]
            .as_object()
            .and_then(|nets| nets.values().next())
            .and_then(|n| n["IPAddress"].as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        containers.push(QuadletContainer {
            name,
            image,
            environment: env,
            volumes,
            publish_ports,
            network,
            ip,
            ..Default::default()
        });
    }

    Ok(containers)
}

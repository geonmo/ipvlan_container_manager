//! 생성된 산출물을 **실제 배포 도구**로 검증하는 통합 테스트.
//!
//! 단위 테스트는 "우리가 의도한 문자열이 나왔는가"만 확인한다. 그 방식으로는
//! 의도 자체가 틀렸을 때를 못 잡는다 — 실제로 이 저장소에는 기본 설정으로
//! 만든 `.res`가 `on-io-error passthrough`(DRBD 9 무효값) 때문에 `drbdadm`에
//! 거부당하는 버그가 있었고, 문자열 일치 테스트는 전부 통과하고 있었다.
//!
//! 그래서 여기서는 생성물을 **그것을 실제로 소비할 도구에게 먹여 본다**:
//!
//! | 산출물 | 검증기 |
//! |---|---|
//! | DRBD `.res` | `drbdadm -c <file> dump <resource>` |
//! | nftables `.nft` | `nft -c -f <file>` (netns 안) |
//! | Ansible 플레이북 | `ansible-playbook --syntax-check` |
//! | Quadlet 유닛 | `podman quadlet --dryrun` |
//!
//! ## 도구가 없는 환경
//!
//! 도구가 없거나 이 환경에서 동작하지 않으면 해당 테스트는 `[SKIP]`을 찍고
//! 지나간다. Rust 테스트 하니스에는 skip 상태가 없어 통과와 구분되지 않으므로,
//! 건너뛴 이유를 반드시 stderr로 남긴다. 어떤 검증기가 실제로 돌았는지는
//! `cargo test -- --nocapture external_validation` 로 확인할 수 있고,
//! `report_external_validator_availability` 테스트가 요약을 출력한다.
//!
//! ## negative control
//!
//! 각 테스트는 본 검증 **앞에** 일부러 잘못된 입력을 같은 검증기에 통과시켜
//! 본다. 이게 없으면 검증기가 권한 문제 등으로 조용히 no-op 할 때 테스트가
//! 헛돌면서 통과한다 (`nft -c`는 권한이 없으면 문법과 무관하게 실패하고,
//! `nft`는 중복 set 원소처럼 잘못 보이는 입력을 에러 없이 병합한다).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

// ─── 공통 유틸 ──────────────────────────────────────────────────────────────

static TMP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 테스트별 임시 작업 디렉토리 (Drop에서 정리).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "icm-extval-{}-{}-{}",
            std::process::id(),
            tag,
            seq
        ));
        std::fs::create_dir_all(&path).expect("임시 디렉토리 생성 실패");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("임시 파일 쓰기 실패");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 명령 실행 결과.
struct Run {
    ok: bool,
    text: String,
}

/// PATH와 표준 sbin 경로에서 실행 파일을 찾는다.
/// (`drbdadm`/`nft`는 보통 `/usr/sbin`에 있어 일반 사용자 PATH에서 빠질 수 있다.)
fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return if p.is_file() { Some(p) } else { None };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/sbin", "/usr/local/sbin"].iter().map(PathBuf::from))
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

fn exec(mut cmd: Command) -> Run {
    // stdin을 막아 둔다 — 어떤 도구도 프롬프트에서 멈추면 안 된다.
    match cmd.stdin(Stdio::null()).output() {
        Ok(o) => Run {
            ok: o.status.success(),
            text: format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        },
        Err(e) => Run {
            ok: false,
            text: format!("실행 실패: {}", e),
        },
    }
}

fn run(program: &Path, args: &[&str]) -> Run {
    let mut cmd = Command::new(program);
    cmd.args(args);
    exec(cmd)
}

fn run_shell(script: &str) -> Run {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    exec(cmd)
}

/// 건너뛴 검증을 반드시 눈에 보이게 남긴다.
fn skip(what: &str, why: &str) {
    eprintln!("[SKIP] {} — {} (이 검증은 실행되지 않았습니다)", what, why);
}

fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

// ─── DRBD .res → drbdadm ────────────────────────────────────────────────────

use crate::models::drbd::{AnsibleInventory, DrbdNode, DrbdResource};

/// `drbdadm dump <res>`는 로컬 호스트명이 `on` 블록에 있어야 리소스를 인식한다.
/// 그래서 첫 노드를 이 머신 호스트명으로 만든다.
fn drbd_fixture() -> DrbdResource {
    let lvm = |hostname: &str, ip: &str| DrbdNode {
        hostname: hostname.to_string(),
        ip: ip.to_string(),
        port: 7789,
        disk_type: "lvm".to_string(),
        lvm_vg: "vg_icmtest".to_string(),
        lvm_size: "10G".to_string(),
        ..DrbdNode::default()
    };
    DrbdResource {
        resource_name: "r0".to_string(),
        nodes: vec![
            lvm(&local_hostname(), "10.99.0.1"),
            lvm("icm-peer2", "10.99.0.2"),
            lvm("icm-peer3", "10.99.0.3"),
        ],
        ..DrbdResource::default()
    }
}

fn drbdadm_check(drbdadm: &Path, dir: &TempDir, name: &str, content: &str) -> Run {
    let f = dir.write(name, content);
    run(drbdadm, &["-c", f.to_str().unwrap(), "dump", "r0"])
}

#[test]
fn generated_res_file_is_accepted_by_drbdadm() {
    let Some(drbdadm) = which("drbdadm") else {
        skip("DRBD .res 검증", "drbdadm 미설치");
        return;
    };
    let dir = TempDir::new("drbd");
    let resource = drbd_fixture();
    let generated = crate::generators::drbd::generate_res_file(&resource);

    // negative control: DRBD 9가 거부하는 값을 끼워 넣은 파일은 반드시 실패해야 한다.
    let broken = generated.replace("on-io-error detach", "on-io-error passthrough");
    assert_ne!(
        broken, generated,
        "negative control을 만들지 못했다 — 기본 on-io-error 값이 바뀐 듯하다"
    );
    let neg = drbdadm_check(&drbdadm, &dir, "negative.res", &broken);
    assert!(
        !neg.ok,
        "negative control 실패: drbdadm이 무효값(on-io-error passthrough)을 통과시켰다.\n\
         이 검증은 신뢰할 수 없다:\n{}",
        neg.text
    );

    // 본 검증
    let out = drbdadm_check(&drbdadm, &dir, "r0.res", &generated);
    assert!(
        out.ok,
        "생성된 .res를 drbdadm이 거부했다:\n{}\n--- 생성물 ---\n{}",
        out.text, generated
    );
}

// ─── nftables .nft → nft -c ─────────────────────────────────────────────────

use crate::models::nft::{
    NftGlobalRule, NftPolicy, NftServiceDef, NftSubnetGroup, NftTarget, NftTargetRule,
};

/// 검증용 더미 인터페이스 이름 (IFNAMSIZ 15자 제한 안).
const NFT_TEST_DEV: &str = "icmveth0";

/// netdev ingress 체인은 실제 존재하는 인터페이스를 요구하므로, 사용자
/// 네트워크 네임스페이스 안에 같은 이름의 dummy 인터페이스를 만든 뒤 검사한다.
/// (호스트의 방화벽은 전혀 건드리지 않는다 — netns 안에서만 동작한다.)
fn nft_check(nft: &Path, file: &Path, device: &str) -> Run {
    run_shell(&format!(
        "unshare -rn sh -c 'ip link add {dev} type dummy && exec {nft} -c -f {file}'",
        dev = device,
        nft = nft.display(),
        file = file.display(),
    ))
}

fn nft_probe_good(dev: &str) -> String {
    format!(
        "table netdev icmtest {{\n    \
         chain c {{\n        \
         type filter hook ingress device \"{dev}\" priority -500; policy accept;\n        \
         ip daddr 10.99.0.1 tcp dport 22 accept\n    \
         }}\n}}\n"
    )
}

fn nft_probe_bad(dev: &str) -> String {
    // 닫히지 않은 set — 명백한 문법 오류
    format!(
        "table netdev icmtest {{\n    \
         chain c {{\n        \
         type filter hook ingress device \"{dev}\" priority -500; policy accept;\n        \
         ip daddr 10.99.0.1 tcp dport {{ 22, accept\n    \
         }}\n}}\n"
    )
}

fn nft_fixture(device: &str) -> NftPolicy {
    let svc = |name: &str, tcp: &[&str]| NftServiceDef {
        id: 0,
        name: name.to_string(),
        description: String::new(),
        tcp_ports: tcp.iter().map(|s| s.to_string()).collect(),
        udp_ports: Vec::new(),
    };
    NftPolicy {
        filename: "filter_ingress.nft".to_string(),
        table_name: "filter_ingress".to_string(),
        device_name: device.to_string(),
        chain_name: format!("ingress_{}", device),
        traceroute_start: 33434,
        traceroute_end: 65535,
        global_rules: vec![NftGlobalRule {
            protocol: "udp".to_string(),
            port_start: 33434,
            port_end: 65535,
        }],
        // 듀얼스택 + 서브넷 그룹 제한 + 전체 허용 규칙 + 대상별 drop 로그를 모두 태운다.
        targets: vec![NftTarget {
            name: "web".to_string(),
            ipv4_addrs: vec!["10.99.0.10".to_string(), "10.99.0.11".to_string()],
            ipv6_addrs: vec!["2001:db8::10".to_string()],
            rules: vec![
                NftTargetRule {
                    service_name: "ssh".to_string(),
                    subnet_group: Some("trusted".to_string()),
                },
                NftTargetRule {
                    service_name: "http".to_string(),
                    subnet_group: None,
                },
            ],
        }],
        subnet_groups: vec![NftSubnetGroup {
            id: 0,
            name: "trusted".to_string(),
            description: String::new(),
            cidrs_v4: vec!["10.99.0.0/24".to_string()],
            cidrs_v6: vec!["2001:db8::/64".to_string()],
        }],
        services: vec![svc("ssh", &["22"]), svc("http", &["80", "443"])],
    }
}

#[test]
fn generated_nft_policy_is_accepted_by_nft() {
    let Some(nft) = which("nft") else {
        skip("nftables 정책 검증", "nft 미설치");
        return;
    };
    if which("unshare").is_none() || which("ip").is_none() {
        skip("nftables 정책 검증", "unshare 또는 ip 미설치");
        return;
    }
    let dir = TempDir::new("nft");

    // 환경 확인 겸 positive control.
    // 권한이 없으면 nft는 문법과 무관하게 netlink 오류로 실패하므로,
    // 정상 파일조차 통과하지 못하면 이 환경에서는 검증 자체가 불가능하다.
    let good = dir.write("probe-good.nft", &nft_probe_good(NFT_TEST_DEV));
    let probe = nft_check(&nft, &good, NFT_TEST_DEV);
    if !probe.ok {
        skip(
            "nftables 정책 검증",
            &format!(
                "이 환경에서 nft 검사를 실행할 수 없음(사용자 네임스페이스/권한): {}",
                probe.text.trim()
            ),
        );
        return;
    }

    // negative control
    let bad = dir.write("probe-bad.nft", &nft_probe_bad(NFT_TEST_DEV));
    assert!(
        !nft_check(&nft, &bad, NFT_TEST_DEV).ok,
        "negative control 실패: nft가 문법 오류를 통과시켰다 — 이 검증은 신뢰할 수 없다"
    );

    // 본 검증
    let policy = nft_fixture(NFT_TEST_DEV);
    let generated = crate::generators::nft::generate_nft_policy(&policy);
    let f = dir.write("policy.nft", &generated);
    let out = nft_check(&nft, &f, NFT_TEST_DEV);
    assert!(
        out.ok,
        "생성된 nft 정책을 nft가 거부했다:\n{}\n--- 생성물 ---\n{}",
        out.text, generated
    );
}

// ─── Quadlet 유닛 → podman quadlet --dryrun ─────────────────────────────────

use crate::models::quadlet::{
    PodNetworkEntry, QuadletConfig, QuadletContainer, QuadletNetwork, QuadletPod, QuadletVolume,
};

fn quadlet_binary() -> Option<PathBuf> {
    which("/usr/libexec/podman/quadlet")
        .or_else(|| which("/usr/lib/podman/quadlet"))
        .or_else(|| which("quadlet"))
}

/// 유닛 파일이 들어 있는 디렉토리를 quadlet 생성기에 통째로 먹인다.
fn quadlet_check(quadlet: &Path, unit_dir: &Path) -> Run {
    let mut cmd = Command::new(quadlet);
    cmd.arg("--dryrun").env("QUADLET_UNIT_DIRS", unit_dir);
    exec(cmd)
}

fn quadlet_fixture() -> QuadletConfig {
    let mut config = QuadletConfig::new();
    config.volumes.push(QuadletVolume {
        name: "icmdata".to_string(),
        driver: "local".to_string(),
        options: Vec::new(),
        labels: Vec::new(),
    });
    config.networks.push(QuadletNetwork {
        name: "ipvlan0".to_string(),
        driver: "ipvlan".to_string(),
        // 실제 배포되는 형태를 검증한다 — 인터페이스가 비면 생성기가
        // `__PARENT_IFACE__` 자리표시자를 넣는데, 그건 Ansible이 치환하기
        // 전의 중간 산출물이라 검증 대상이 아니다.
        interface: "eth0".to_string(),
        subnet: "10.99.0.0/24".to_string(),
        gateway: "10.99.0.1".to_string(),
        subnet6: "2001:db8::/64".to_string(),
        gateway6: "2001:db8::1".to_string(),
        ipv6: true,
        ipvlan_mode: "l2".to_string(),
        internal: false,
        options: Vec::new(),
    });
    config.pods.push(QuadletPod {
        name: "icmpod".to_string(),
        description: Some("ICM 검증용 Pod".to_string()),
        hostname: Some("icmpod".to_string()),
        shm_size: Some("64m".to_string()),
        networks: vec![PodNetworkEntry {
            network: "ipvlan0".to_string(),
            ip: Some("10.99.0.10".to_string()),
            ip6: Some("2001:db8::10".to_string()),
            gateway: Some("10.99.0.1".to_string()),
            gateway6: Some("2001:db8::1".to_string()),
        }],
        labels: vec!["app=icm".to_string()],
    });
    config.containers.push(QuadletContainer {
        name: "icmapp".to_string(),
        image: "quay.io/fedora/fedora:latest".to_string(),
        description: Some("ICM 검증용 컨테이너".to_string()),
        pod: Some("icmpod".to_string()),
        environment: vec![("ICM_ENV".to_string(), "test".to_string())],
        volumes: vec!["icmdata:/data:z".to_string()],
        add_capabilities: vec!["NET_ADMIN".to_string()],
        ..QuadletContainer::default()
    });
    config
}

#[test]
fn generated_quadlet_units_are_accepted_by_podman_quadlet() {
    let Some(quadlet) = quadlet_binary() else {
        skip("Quadlet 유닛 검증", "podman quadlet 생성기를 찾을 수 없음");
        return;
    };

    // negative control: Image= 없는 .container는 반드시 거부돼야 한다.
    let neg_dir = TempDir::new("quadlet-neg");
    neg_dir.write("broken.container", "[Container]\nContainerName=broken\n");
    let neg = quadlet_check(&quadlet, neg_dir.path());
    assert!(
        !neg.ok,
        "negative control 실패: quadlet이 Image= 없는 유닛을 통과시켰다 — 이 검증은 신뢰할 수 없다:\n{}",
        neg.text
    );

    // 본 검증: 생성기가 만든 유닛 전체를 한 디렉토리에 넣고 돌린다.
    let dir = TempDir::new("quadlet");
    let config = quadlet_fixture();
    let files = crate::generators::quadlet::generate_all_units(&config);
    assert!(!files.is_empty(), "생성된 유닛 파일이 없다");
    let mut rendered = String::new();
    for (name, content) in &files {
        dir.write(name, content);
        rendered.push_str(&format!("--- {} ---\n{}\n", name, content));
    }

    let out = quadlet_check(&quadlet, dir.path());
    assert!(
        out.ok,
        "생성된 Quadlet 유닛을 podman quadlet이 거부했다:\n{}\n{}",
        out.text, rendered
    );
}

// ─── Ansible 플레이북 → ansible-playbook --syntax-check ─────────────────────

fn ansible_check(ap: &Path, dir: &TempDir, name: &str, playbook: &str) -> Run {
    let f = dir.write(name, playbook);
    run(ap, &["--syntax-check", "-i", "localhost,", f.to_str().unwrap()])
}

/// 각 탭이 만드는 플레이북 (LINSTOR는 외부 컬렉션이 필요해 별도 테스트).
fn generated_playbooks() -> Vec<(&'static str, String)> {
    let resource = drbd_fixture();
    let drbd = crate::generators::drbd::generate_ansible_playbook(&resource, &AnsibleInventory::default());

    let quadlet_config = quadlet_fixture();
    let quadlet_files = crate::generators::quadlet::generate_all_units(&quadlet_config);
    let quadlet_form = crate::routes::quadlet::QuadletFullForm {
        net_driver: None,
        net_interface: None,
        net_subnet: None,
        net_gateway: None,
        net_subnet6: None,
        net_gateway6: None,
        net_ipvlan_mode: None,
        pod_name: Some("icmpod".to_string()),
        pod_description: None,
        pod_hostname: None,
        pod_shm_size: None,
        pod_networks_json: None,
        ansible_hosts: Some("node1,node2,node3".to_string()),
        ansible_user: Some("deploy".to_string()),
        ansible_ssh_key: Some("~/.ssh/id_rsa".to_string()),
    };
    let quadlet = crate::routes::quadlet::generate_quadlet_ansible_playbook(
        &quadlet_config,
        &quadlet_files,
        &quadlet_form,
    );

    let nft = crate::generators::nft::generate_nft_ansible_playbook(
        &nft_fixture(NFT_TEST_DEV),
        "node1,node2,node3",
        "deploy",
        "~/.ssh/id_rsa",
    );

    let pacemaker = crate::routes::pacemaker::generate_pacemaker_ansible_playbook(
        &pacemaker_form(),
        &pacemaker_config(),
    );

    vec![
        ("drbd", drbd),
        ("quadlet", quadlet),
        ("nft", nft),
        ("pacemaker", pacemaker),
    ]
}

use crate::models::pacemaker::{
    ClusterNode, DrbdPacemakerResource, FsResource, PacemakerConfig, ResourceGroup, StonithDevice,
    SystemdResource,
};
use crate::routes::pacemaker::PacemakerFormData;

fn pacemaker_form() -> PacemakerFormData {
    PacemakerFormData {
        cluster_name: "ha-cluster".to_string(),
        nodes_json: None,
        stonith_enabled: Some("on".to_string()),
        no_quorum_policy: "stop".to_string(),
        migration_threshold: None,
        failure_timeout: None,
        drbd_resources_json: None,
        systemd_resources_json: None,
        resource_groups_json: None,
        stonith_devices_json: None,
        auto_constraints: Some("on".to_string()),
        order_constraints_json: None,
        colocation_constraints_json: None,
        location_constraints_json: None,
        ansible_hosts: Some("node1,node2,node3".to_string()),
        ansible_user: Some("deploy".to_string()),
        ansible_ssh_key: Some("~/.ssh/id_rsa".to_string()),
    }
}

/// 부트스트랩 + STONITH + 리소스/제약조건 play를 모두 켠 구성.
fn pacemaker_config() -> PacemakerConfig {
    let mut config = PacemakerConfig::default();
    config.cluster.cluster_name = "ha-cluster".to_string();
    config.cluster.nodes = (1..=3)
        .map(|i| ClusterNode {
            name: format!("node{}", i),
            id: i,
        })
        .collect();
    config.drbd_resources.push(DrbdPacemakerResource {
        on_fail: "fence".to_string(),
        ..DrbdPacemakerResource::default()
    });
    config.fs_resources.push(FsResource {
        drbd_clone_name: "drbd-r0-clone".to_string(),
        ..FsResource::default()
    });
    config.systemd_resources.push(SystemdResource {
        resource_name: "svc-icmpod".to_string(),
        systemd_unit: "icmpod-pod.service".to_string(),
        ..SystemdResource::default()
    });
    config.resource_groups.push(ResourceGroup {
        group_name: "grp-icm".to_string(),
        members: vec!["svc-icmpod".to_string()],
        after_fs: Some("fs-r0".to_string()),
    });
    config.stonith_devices.push(StonithDevice {
        node: "node1".to_string(),
        ipmi_ip: "10.99.1.1".to_string(),
        ipmi_user: "admin".to_string(),
        ipmi_password: "dummy".to_string(),
        extra_opts: Vec::new(),
    });
    crate::generators::pacemaker::generate_default_constraints(&mut config);
    config
}

#[test]
fn generated_ansible_playbooks_pass_syntax_check() {
    let Some(ap) = which("ansible-playbook") else {
        skip("Ansible 플레이북 검증", "ansible-playbook 미설치");
        return;
    };
    let dir = TempDir::new("ansible");

    // negative control: 깨진 YAML은 반드시 거부돼야 한다.
    let neg = ansible_check(
        &ap,
        &dir,
        "negative.yml",
        "---\n- name: broken\n  hosts: a\nb\n  tasks: []\n",
    );
    assert!(
        !neg.ok,
        "negative control 실패: ansible-playbook이 깨진 YAML을 통과시켰다 — 이 검증은 신뢰할 수 없다:\n{}",
        neg.text
    );

    for (name, playbook) in generated_playbooks() {
        let out = ansible_check(&ap, &dir, &format!("{}.yml", name), &playbook);
        assert!(
            out.ok,
            "{} 플레이북이 --syntax-check를 통과하지 못했다:\n{}\n--- 생성물 ---\n{}",
            name, out.text, playbook
        );
    }
}

// ─── LINSTOR (외부 컬렉션 필요) ─────────────────────────────────────────────

use crate::models::linstor::{
    LinstorConfig, LinstorResourceGroup, LinstorResourceSpawn, LinstorStoragePool,
};

fn linstor_fixture() -> LinstorConfig {
    LinstorConfig {
        controller_nodes: vec!["node1".to_string()],
        satellite_nodes: vec!["node1".to_string(), "node2".to_string(), "node3".to_string()],
        rpm_dir: "/opt/linstor-rpms".to_string(),
        storage_pools: vec![LinstorStoragePool {
            name: "sp-lvmthin".to_string(),
            pool_type: "lvmthin".to_string(),
            vg: Some("vg_linstor".to_string()),
            vg_thinpool: Some("thinpool".to_string()),
            zpool: None,
            file_path: None,
            physical_devices: vec!["/dev/sdb".to_string()],
            nodes: vec!["node1".to_string(), "node2".to_string(), "node3".to_string()],
        }],
        resource_groups: vec![LinstorResourceGroup {
            name: "rg-app".to_string(),
            storage_pool: "sp-lvmthin".to_string(),
            place_count: 3,
        }],
        resources: vec![LinstorResourceSpawn {
            name: "res-app".to_string(),
            resource_group: "rg-app".to_string(),
            size: "100G".to_string(),
        }],
        deploy_storage: true,
        ha_database: false,
        token_auth: true,
    }
}

/// `linbit.linstor` 컬렉션이 설치돼 있는지 확인한다.
/// 생성된 플레이북은 `import_role: linbit.linstor.cluster_init`을 쓰는데,
/// `--syntax-check`는 role을 **파싱 시점에 실제로 찾으므로** 컬렉션이 없으면
/// 우리 생성물과 무관하게 실패한다.
fn linbit_collection_installed() -> bool {
    let Some(galaxy) = which("ansible-galaxy") else {
        return false;
    };
    let out = run(&galaxy, &["collection", "list", "linbit.linstor"]);
    // `ansible-galaxy collection list`는 컬렉션이 없어도 rc=0으로 끝내고
    // "unable to find ... in collection paths" 경고만 찍는다. 종료 코드만
    // 보면 항상 설치된 것으로 오판한다.
    out.ok && !out.text.contains("unable to find") && out.text.contains("linbit.linstor")
}

#[test]
fn generated_linstor_playbook_passes_syntax_check() {
    let Some(ap) = which("ansible-playbook") else {
        skip("LINSTOR 플레이북 검증", "ansible-playbook 미설치");
        return;
    };
    if !linbit_collection_installed() {
        skip(
            "LINSTOR 플레이북 검증",
            "linbit.linstor 컬렉션 미설치 — 생성된 requirements.yml로 \
             `ansible-galaxy collection install -r requirements.yml` 후 다시 실행하세요",
        );
        return;
    }

    let dir = TempDir::new("linstor");
    let config = linstor_fixture();
    let playbook =
        crate::generators::linstor::generate_linstor_ansible_playbook(&config, "deploy", "~/.ssh/id_rsa");
    let out = ansible_check(&ap, &dir, "linstor.yml", &playbook);
    assert!(
        out.ok,
        "LINSTOR 플레이북이 --syntax-check를 통과하지 못했다:\n{}\n--- 생성물 ---\n{}",
        out.text, playbook
    );
}

/// `ansible-inventory`는 파일을 파싱하지 못해도 **rc=0으로 끝내고** 경고만
/// 찍는다. 종료 코드만 보면 어떤 쓰레기 입력이든 통과하므로, 경고 문구를
/// 실패로 취급해야 검증이 의미를 갖는다.
fn inventory_graph(ai: &Path, file: &Path) -> Run {
    let out = run(ai, &["-i", file.to_str().unwrap(), "--graph"]);
    Run {
        ok: out.ok && !out.text.contains("Unable to parse"),
        text: out.text,
    }
}

/// LINSTOR 인벤토리는 외부 컬렉션 없이도 검증할 수 있다
/// (`ansible-inventory --graph`가 그룹 구조까지 확인해 준다).
#[test]
fn generated_linstor_inventory_is_readable_by_ansible() {
    let Some(ai) = which("ansible-inventory") else {
        skip("LINSTOR 인벤토리 검증", "ansible-inventory 미설치");
        return;
    };
    let dir = TempDir::new("linstor-inv");

    // negative control
    let bad = dir.write("bad.yml", "all:\n  hosts:\n   - [unbalanced\n");
    assert!(
        !inventory_graph(&ai, &bad).ok,
        "negative control 실패: ansible-inventory가 깨진 인벤토리를 통과시켰다 — 이 검증은 신뢰할 수 없다"
    );

    let config = linstor_fixture();
    let node_ips = std::collections::HashMap::from([
        ("node1".to_string(), "10.99.0.1".to_string()),
        // 노드 풀에 IP가 비어 있는 경우도 함께 태운다
        ("node2".to_string(), String::new()),
    ]);
    let inventory = crate::generators::linstor::generate_linstor_inventory(
        &config,
        &node_ips,
        "deploy",
        "~/.ssh/id_rsa",
    );
    let f = dir.write("inventory.yml", &inventory);
    let out = inventory_graph(&ai, &f);
    assert!(
        out.ok,
        "생성된 LINSTOR 인벤토리를 ansible-inventory가 읽지 못했다:\n{}\n--- 생성물 ---\n{}",
        out.text, inventory
    );
    // README가 요구하는 그룹이 실제로 resolve돼야 한다.
    for group in ["linstor_controllers", "linstor_satellites", "linstor_cluster"] {
        assert!(
            out.text.contains(group),
            "인벤토리에 {} 그룹이 보이지 않는다:\n{}",
            group,
            out.text
        );
    }
}

// ─── 어떤 검증기가 실제로 돌았는지 요약 ─────────────────────────────────────

/// 도구가 없어 건너뛴 검증을 "통과"로 오해하지 않도록 가용성을 출력한다.
/// `cargo test -- --nocapture external_validation` 으로 확인한다.
#[test]
fn report_external_validator_availability() {
    let rows: Vec<(&str, bool, &str)> = vec![
        ("drbdadm (.res)", which("drbdadm").is_some(), "drbd9x-utils"),
        ("nft (.nft)", which("nft").is_some(), "nftables"),
        (
            "podman quadlet (유닛)",
            quadlet_binary().is_some(),
            "podman",
        ),
        (
            "ansible-playbook (플레이북)",
            which("ansible-playbook").is_some(),
            "ansible-core",
        ),
        (
            "ansible-inventory (인벤토리)",
            which("ansible-inventory").is_some(),
            "ansible-core",
        ),
        (
            "linbit.linstor 컬렉션",
            linbit_collection_installed(),
            "ansible-galaxy collection install -r requirements.yml",
        ),
    ];
    eprintln!("\n── 외부 검증기 가용성 ──────────────────────────────");
    for (name, available, hint) in &rows {
        eprintln!(
            "  {} {:<32} {}",
            if *available { "O" } else { "-" },
            name,
            if *available { "" } else { hint }
        );
    }
    eprintln!("────────────────────────────────────────────────────\n");
}

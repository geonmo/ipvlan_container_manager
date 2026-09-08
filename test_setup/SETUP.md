# test_setup — ipvlan_container_manager 테스트 클러스터 사전 셋업

`ipvlan_container_manager` 웹앱(DRBD/Quadlet/Pacemaker 설정 생성기)을 실제로 테스트하기 위해
필요한 기반 인프라(Pacemaker 클러스터 + IPVLAN public/private 네트워크)를 3-node VM 환경에
구성하는 Ansible 코드다.

`ansible/playbooks/{deploy_pacemaker,deploy_drbd,deploy_quadlet}.yml`(앱이 사용자에게 보여주는
참조 플레이북)을 최대한 그대로 사용하고, 이 환경에 빠져 있던 부분만 채워 넣었다:

- IPVLAN(public/private) 네트워크 생성 플레이북 (참조 플레이북 3개에는 없음, `playbooks_cmst2/container_playbooks/R00.ipvlan.yml`에서 이식)
- 인벤토리 / group_vars (호스트, 접속 계정, 클러스터 이름, hacluster 비밀번호, 서브넷)
- `deploy_pacemaker.yml`이 요구하는 `files/pacemaker_setup.sh` 부트스트랩 스크립트 (corosync는 private 망으로 통신하도록 `addr=` 고정)
- 이 VM 이미지 특유의 `/etc/hosts` 자기참조 문제 수정 플레이북
- 이 VM 이미지 특유의 firewalld `internal` 존 ipset(work4) 대상 대역 교체 플레이북 (corosync가 private 망을 쓰도록)
- `deploy_pacemaker.yml`에 빠져 있던 저장소 활성화(crb/highavailability) 및 방화벽 존 보완

프로덕션(`playbooks_cmst2`)과 달리 이 환경은 SSH 키 인증 + 무비밀번호 sudo가 이미 되어 있으므로
gsdcadmin 키 배포(R02.setup_sshd.yml) 같은 인증 관련 플레이북은 옮기지 않았다.

## 환경 정보 (실제 사용한 값)

### 노드 (`/etc/hosts` 기준)

| 호스트 | eth0 (관리망, DHCP) | eth1 (public 매핑) | eth2 (private 매핑) |
|--------|--------------------|--------------------|-----------------------|
| node01.build.test | (관리망 DHCP, 생략) | 198.51.100.11/24 | 192.0.2.11/24 |
| node02.build.test | (관리망 DHCP, 생략) | 198.51.100.12/24 | 192.0.2.12/24 |
| node03.build.test | (관리망 DHCP, 생략) | 198.51.100.13/24 | 192.0.2.13/24 |

- OS: AlmaLinux 9.6, SELinux Enforcing, firewalld 활성
- 접속 계정: `geonmo` (SSH 키 인증 + `sudo -n` 무비밀번호 확인됨)
- eth0(DHCP 관리망, 생략)는 사용하지 않음

### IPVLAN 네트워크 매핑 근거

프로덕션(R00.ipvlan.yml)은 실제 인터넷 대역(public) / 별도 사설 대역(private)을 각각
`ansible.utils.ipaddr` 필터로 자동 탐지한 인터페이스에 붙인다. 이 테스트 환경에는 그 대역이
없으므로, 실제 존재하는 두 개의 노드간 통신망을 그대로 매핑했다 (아래 표는 이 테스트 환경
전용 값 — 문서 예시 대역으로 표기):

| 역할 | 서브넷 | 부모 인터페이스 | 비고 |
|------|--------|------------------|------|
| public | `198.51.100.0/24` | eth1 (자동 탐지) | `/etc/hosts`에 등록된 노드 identity망 |
| private | `192.0.2.0/24` | eth2 (자동 탐지) | Internal=true, mode=l2 (게이트웨이 없는 격리망) |

두 대역 모두 실제 라우터가 없는 host-only 성격이라 `Gateway=`는 설정하지 않았고, IPv6 주소가
없는 환경이라 public 쪽 `IPv6=true`/IPv6 Subnet도 넣지 않았다(private는 원본과 동일하게
`IPv6=no`).

생성된 파일 (`/etc/containers/systemd/`, 모든 노드 공통):

```ini
# public-ipvlan.network
[Network]
Driver=ipvlan
Subnet=198.51.100.0/24
Options=parent=eth1

# private-ipvlan.network
[Network]
Driver=ipvlan
Subnet=192.0.2.0/24
IPv6=no
Options=parent=eth2
Options=mode=l2
Internal=true
```

> **버그 수정**: 원본 `R00.ipvlan.yml`은 `Options=parent={{ private_interface }},mode=l2` 처럼
> 콤마로 옵션을 묶었는데, quadlet의 `Options=`는 (`podman network create --opt`와 동일하게)
> **한 줄에 key=value 하나만** 허용한다. 콤마로 묶으면 `parent` 값 자체가
> `"eth2,mode=l2"`라는 잘못된 문자열이 되어 `podman network create` 단계에서
> `parent interface eth2,mode=l2 does not exist` 오류로 실패한다(private 쪽만 이 문제가
> 있었음 — public은 옵션이 1개뿐이라 우연히 문제가 없었다). `Options=`를 두 줄로 분리해서
> 해결했다. **프로덕션의 `R00.ipvlan.yml`도 같은 버그가 있을 가능성이 높으니 확인 필요.**

## Pacemaker 클러스터

| 항목 | 값 |
|------|-----|
| 클러스터 이름 | `test_cluster` |
| 스택 | Corosync(knet) + Pacemaker |
| hacluster 비밀번호 | `TestCluster!2026` (테스트 전용 평문, `group_vars/all.yml`; 운영은 vault 사용) |
| stonith-enabled | `false` (IPMI/fencing 장비 없음) |
| no-quorum-policy | `ignore` |
| corosync 통신 대역 | `192.0.2.0/24` (**private**, eth2) — 노드 이름에 `addr=192.0.2.x`로 명시 고정 |

> corosync는 반드시 private 네트워크로만 통신해야 한다는 요구사항에 따라, public
> 네트워크(198.51.100.0/24, 노드 이름 해석용)와 분리했다. `pcs cluster setup`의
> `NODENAME addr=IP` 문법으로 노드 식별자(이름)는 그대로 두고 실제 corosync ring 주소만
> private IP로 고정했다.

### `/etc/hosts` 자기참조 버그

이 VM 이미지는 각 노드가 자기 자신의 FQDN을 두 번 등록한다:

```
127.0.1.1 node01.build.test node01      # vagrant 기본 (자기 자신 전용)
...
198.51.100.11 node01.build.test         # vagrant-hostmanager 블록 (전체 노드 공용)
```

glibc 리졸버는 첫 매치를 쓰므로 **node01 자기 자신 입장에서만** `node01.build.test`가
`127.0.1.1`로 해석되고, node02/node03 입장에서는 `198.51.100.11`으로 해석된다. 이 상태로
`pcs cluster setup`을 이름만 넘겨 실행하면 corosync가 노드마다 서로 다른 주소로 바인딩되어
멤버십이 맺어지지 않는다(`pcs status`에서 전 노드가 계속 `OFFLINE`, `corosync-cfgtool -s`의
로컬 addr이 `127.0.1.1`로 표시됨).

`00-fix-etc-hosts.yml`이 관리 대상이 아닌 `127.0.1.1 ...` 줄만 제거한다(`127.0.0.1`
localhost 줄은 그대로 둠). 실제 노드간 통신에 쓰이는 `198.51.100.0/24` 블록은
vagrant-hostmanager가 계속 관리하므로 건드리지 않았다. 추가로 `pacemaker_setup.sh`에서도
`pcs cluster setup`에 각 노드의 `addr=198.51.100.x`를 명시해 이중으로 방지해 두었다.

### 방화벽 존(zone) 함정

`firewall-cmd --get-active-zones`로 보면 이 VM들은 사전 구성된 `internal` 존이
`ipset:work4`(최초 내용: `172.16.0.0/12`, `198.51.100.0/24`) 소스를 갖고 있어, **소스 IP
기준 매치가 인터페이스 기준 매치보다 우선**한다. 즉 `198.51.100.0/24`(eth1, public으로
매핑한 그 대역)에서 오는 트래픽은 기본 존(`public`)이 아니라 `internal` 존 규칙을 탄다.
`internal` 존은 `ssh/cockpit/mdns/samba-client`만 허용해서, `public` 존에만
`high-availability` 서비스를 추가하면 실제로는 노드간 corosync/pcsd 트래픽이 막혀버린다
(`pcs host auth` 단계에서 `Unable to communicate with nodeXX` / curl에서 `No route to host`).

`deploy_pacemaker.yml`의 방화벽 태스크를 `public`, `internal` 두 존 모두에
`high-availability` 서비스를 허용하도록 수정했다.

### ipset(work4) 대역 교체 — corosync를 private 망으로

corosync가 `192.0.2.0/24`(private, eth2)로 통신하도록 바꾸면서, "cluster-internal 트래픽은
`internal` 존을 타게 한다"는 이 환경의 설계 의도에 맞춰 `internal` 존의 ipset 대상도
`198.51.100.0/24` → `192.0.2.0/24` 로 교체했다(`00-fix-firewalld-ipset.yml`). 결과:

- `198.51.100.0/24`(public, 노드 이름/SSH/ansible 관리용) → 이제 인터페이스 매치로 **`public`
  존**을 탄다.
- `192.0.2.0/24`(private, corosync) → **`internal`** 존(ipset)을 탄다.

> **사고 및 교훈**: `198.51.100.0/24`를 ipset에서 빼는 순간 SSH 접속이 전 노드에서 즉시
> 끊겼다. 알고 보니 이 환경은 **ssh 서비스가 `public` 존이 아니라 `internal` 존에서만
> 허용되고 있었다**(`public` 존 서비스 목록에는 애초부터 `ssh`가 없었음). 즉
> `198.51.100.0/24`가 `internal` 존을 타는 동안에만 그 대역의 SSH가 허용되고 있었던
> 것 — 그 대역을 ipset에서 빼자마자 ssh 미허용인 `public` 존으로 넘어가며 잠겼다.
> (다행히 eth0 관리망 IP와 private IP(192.0.2.x)로는 SSH 포트 자체는 열려 있어 host key
> 미등록 상태였을 뿐이라 그 경로로 복구했다.) 그래서 `00-fix-firewalld-ipset.yml`은
> **ipset을 건드리기 전에 먼저 `public` 존에 `ssh` 서비스를 영구 허용**해 두고, 그 다음에
> ipset 항목을 교체하는 순서로 작성했다 — 순서를 바꾸면 재현 시 다시 잠길 수 있으니 주의.

## 디렉토리 구조

```
test_setup/
├── ansible.cfg              # inventory 경로, pipelining 등
├── inventory.ini            # node01~03.build.test (별도 그룹 불필요, 전부 hosts: all)
├── group_vars/
│   └── all.yml               # quadlet_dir, public/private_subnet, cluster_name, hacluster_password
├── playbooks/
│   ├── 00-fix-etc-hosts.yml         # (신규) 127.0.1.1 자기참조 라인 제거
│   ├── 00-fix-firewalld-ipset.yml   # (신규) internal 존 ipset(work4) 대역을 192.0.2.0/24로 교체 + ssh 잠금 방지
│   ├── 01-ipvlan-networks.yml       # (신규, R00.ipvlan.yml 이식) public/private ipvlan .network 생성
│   ├── deploy_pacemaker.yml         # (ansible/playbooks/ 원본 + crb/ha 저장소 활성화, 방화벽 존 보완)
│   ├── deploy_drbd.yml              # (ansible/playbooks/ 원본, 무수정 — 앱에서 리소스별로 생성 후 사용)
│   ├── deploy_quadlet.yml           # (ansible/playbooks/ 원본, 무수정 — 앱에서 유닛 생성 후 사용)
│   └── files/
│       └── pacemaker_setup.sh       # (신규) test_cluster 부트스트랩 스크립트 (addr=192.0.2.x 명시)
└── SETUP.md                        # 이 문서
```

`deploy_drbd.yml`/`deploy_quadlet.yml`은 손대지 않았다. 이 두 플레이북은 앱의 `/drbd/`,
`/quadlet/` 탭에서 실제 리소스(res 파일, 컨테이너 유닛)를 만든 뒤 `-e` 변수로 내용을 넘겨
실행하는 용도라, 지금 단계(사전 셋업)에서는 실행 대상이 없다.

## 실행 방법

```bash
cd ipvlan_container_manager/test_setup

# 1) /etc/hosts 자기참조 문제 수정 (최초 1회, 멱등)
ansible-playbook playbooks/00-fix-etc-hosts.yml

# 2) firewalld internal 존 ipset을 private(192.0.2.0/24)로 교체 (ssh 허용을 먼저 public에 심어둠)
ansible-playbook playbooks/00-fix-firewalld-ipset.yml

# 3) IPVLAN public/private 네트워크 생성
ansible-playbook playbooks/01-ipvlan-networks.yml

# 4) Pacemaker 클러스터 부트스트랩 (corosync는 private 망 192.0.2.0/24로 통신)
ansible-playbook playbooks/deploy_pacemaker.yml
```

이후 DRBD/Quadlet 리소스는 `ipvlan_container_manager` 웹 UI(`/drbd/`, `/quadlet/`,
`/pacemaker/`)에서 이 클러스터(노드 3대, `public-ipvlan`/`private-ipvlan` 네트워크)를
대상으로 생성한 뒤 `deploy_drbd.yml` / `deploy_quadlet.yml`로 배포하면 된다.

> 참고: 이 셸/에이전트 샌드박스에서 `ansible`/`ansible-playbook`을 직접 실행하면
> `ERROR: Ansible requires blocking IO on stdin/stdout/stderr` 가 나는 환경이 있었다.
> 그 경우 `script -qec "ansible-playbook ..." /dev/null` 로 감싸서 실행하면 우회된다
> (일반 터미널에서 직접 실행할 때는 필요 없다).

## 검증 결과

```
$ corosync-cfgtool -s
Local node ID 1, transport knet
LINK ID 0 udp
	addr	= 192.0.2.11                 # private 망으로 바인딩됨 (198.51.100.x 아님)
	status:
		nodeid:          1:	localhost
		nodeid:          2:	connected
		nodeid:          3:	connected

$ pcs status
Cluster name: test_cluster
Cluster Summary:
  * Stack: corosync (Pacemaker is running)
  * Current DC: node03.build.test (version 2.1.10-3.el9_8-5693eaeee) - partition with quorum
  * 3 nodes configured
  * 0 resource instances configured

Node List:
  * Online: [ node01.build.test node02.build.test node03.build.test ]

Daemon Status:
  corosync: active/enabled
  pacemaker: active/enabled
  pcsd: active/enabled

$ pcs property show | tail -6
  cluster-infrastructure=corosync
  cluster-name=test_cluster
  dc-version=2.1.10-3.el9_8-5693eaeee
  have-watchdog=false
  no-quorum-policy=ignore
  stonith-enabled=false

$ podman network ls   # 각 노드 공통
NETWORK ID    NAME                    DRIVER
2f259bab93aa  podman                  bridge
xxxxxxxxxxxx  systemd-private-ipvlan  ipvlan
xxxxxxxxxxxx  systemd-public-ipvlan   ipvlan
```

전체 플레이북(`00-fix-etc-hosts` → `00-fix-firewalld-ipset` → `01-ipvlan-networks` →
`deploy_pacemaker`)을 처음부터 다시 실행해도 오류 없이 "이미 구성됨"으로 건너뛰며 클러스터가
그대로 온라인/쿼럼 상태를 유지함을 확인했다(멱등성 확인됨). SSH는 `node0X.build.test`
호스트네임(198.51.100.0/24, public 존)으로 정상 동작한다.

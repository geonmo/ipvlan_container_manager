# IPVLAN Container Manager — 기능 개선 계획

## 배경

이 앱은 `~/playbooks_cmst2/` 하위 Ansible 플레이북(`container_playbooks/`
및 형제 디렉토리의 `R01/R02/R09/R10/R11/R53/R54/R55/R56` 등)이 손으로
작성/유지하던
DRBD + Quadlet + Pacemaker + nftables 구성을 웹 UI로 대체하기 위한 도구다.
그중 `~/playbooks_cmst2/docs/pacemaker-drbd-guide.md`는 2026-07 실제 장애
대응 세션에서 얻은 운영 지식을 정리한 문서로, **문서 자체가 "향후 pacemaker
설정을 자동 생성하는 프로그램(= 이 앱)의 스펙으로 쓰일 것"을 전제로 작성**
되어 있다. 아래 계획은 이 문서를 최우선 근거로 삼고, 그 외 `container_playbooks/`
실물 파일들에서 찾은 범용 인프라 패턴을 보조 근거로 삼는다.
`~/playbooks_cmst2/` 자체는 읽기 전용 참고 자료이며 수정하지 않는다.
기존 코드의 함수/모델을 최대한 재사용하는 방향으로 정리했다.

코드 구조/함수 레퍼런스는 [DEVEL.md](DEVEL.md), 설치/사용법은
[README.md](README.md) 참고.

---

## A. [최우선] Pacemaker 운영 경험 반영 — `docs/pacemaker-drbd-guide.md` +
실제 운영 플레이북 기반

`docs/pacemaker-drbd-guide.md`(8절, 실사고 4건)뿐 아니라, 그 문서가 참고한
**실제 운영 플레이북 원본**까지 직접 대조했다:
`R01.service_pacemaker_firewall.yml`, `R02.service_pacemaker_setup.yml`,
`R02.service_pacemaker_stonith.yml`, `R09.drbd.yml`,
`R10.drbd_volume_and_config.yml`, `R11.pacemaker_drbd_resources.yml`,
`R53.firewalld_pacemaker.yml`, `R54.se_backend_pacemaker.yml`,
`R55.se_backend_stonith.yml`, `R56.cluster_rolling_maintenance.yml`
(모두 `~/playbooks_cmst2/` 최상위 — `container_cluster`와 `se_cluster` 두
클러스터에 동일 패턴이 중복 적용되어 있어 "우연"이 아니라 "표준 패턴"임을
확인할 수 있었다). 아래는 이걸 `src/generators/pacemaker.rs` /
`src/generators/drbd.rs` / `src/models/pacemaker.rs` / `src/routes/pacemaker.rs`
와 항목별로 대조한 결과다.

### A.1 [치명적] DRBD fence-peer 핸들러가 전혀 생성되지 않음

**✅ 구현 완료 (2026-09-04)** — `src/generators/drbd.rs`에
`generate_global_common_conf()` 추가, `generate_ansible_playbook()`에
task 11(조건부: `fencing == "resource-only"`)로 배포 + `drbdadm adjust
all` 핸들러 연결, `/drbd/` 화면에 안내 문구 추가. 단위 테스트 3개
통과(`cargo test drbd::`). 실동작 검증은 VM 3대 테스트 클러스터에서 별도
진행 예정.

**이게 바로 사용자가 언급한 "pacemaker가 primary를 옮겼는데 drbd가 이를
몰라서 활성화 안 되는" 현상의 근본 원인과 일치한다.**

- `src/models/drbd.rs:96-107` `DrbdDiskOptions.fencing`의 **기본값이
  이미 `"resource-only"`**다 (즉 이 앱이 만드는 모든 DRBD 리소스가 기본적으로
  이 정책을 쓴다).
- 가이드 3.2절/4.1절: `fencing resource-only`는 피어 단절 시
  **fence-peer 핸들러가 실행되어야만 promote가 진행**되도록 만드는데, 이
  핸들러는 `.res` 파일이 아니라 노드의 `/etc/drbd.d/global_common.conf`
  `handlers {}` 블록에 별도로 등록해야 한다:
  ```
  common {
      handlers {
          fence-peer "/usr/lib/drbd/crm-fence-peer.9.sh";
          after-resync-target "/usr/lib/drbd/crm-unfence-peer.9.sh";
      }
  }
  ```
  (스크립트 자체는 `drbd9x-utils` 패키지에 이미 포함됨 — 별도 설치 불필요,
  등록만 하면 됨.)
- **현재 `src/generators/drbd.rs`는 `.res` 파일만 생성하고
  (`generate_res_file()`), `global_common.conf`는 어디에서도 생성/배포하지
  않는다.** `generate_ansible_playbook()` (drbd.rs:145-349)도 이 파일을
  다루지 않는다.
- 결과: 이 앱으로 만든 DRBD 구성은 겉보기엔 정상 작동하다가, **피어가
  끊기는 순간(장애/재부팅/standby)에만 promote가 응답 없이 hang** 되고
  pacemaker op timeout으로 실패 처리된다 — 가이드 7.1절 실제 사고와 정확히
  같은 패턴이며, "pcs가 리소스를 옮기라고 지시했는데 drbd 커널 모듈이 이걸
  인지 못 하는 것처럼 보이는" 증상으로 나타난다.
- **실제 운영 플레이북에서 정확히 이 작업을 하고 있는 걸 확인함** —
  `R09.drbd.yml` task 11 (`DRBD 전역 공통 설정(global_common.conf) 배포`)이
  `templates/drbd_global_common.conf.j2`를 `/etc/drbd.d/global_common.conf`
  로 배포하고, 파일에 남은 주석이 이 문제를 그대로 설명한다: *"net {
  fencing resource-only; } 정책이 동작하려면 반드시 필요합니다. 핸들러가
  없으면 피어 단절 시 promote가 core dump 없이 op timeout까지 그대로
  hang 됩니다."* 즉 이 앱이 놓친 기능은 실제 운영 환경에서 이미 별도
  플레이북으로 존재하는, **검증된 필수 기능**이다.
- 배포 태스크는 `notify: Adjust DRBD config` 핸들러와 짝을 이룬다 —
  `drbdadm adjust all` (로컬 설정 파일과 커널 상태의 diff만 반영하는
  멱등적 명령이라 변경 없으면 no-op, 매 배포마다 안전하게 재실행 가능).
  즉 파일만 배포하고 끝나는 게 아니라 **배포 후 반드시 `drbdadm adjust
  all`을 실행해서 살아있는 리소스에 즉시 반영**해야 한다 (재시작 불필요).

**변경 대상**:
- `src/generators/drbd.rs`에 `generate_global_common_conf(resources:
  &[DrbdResource]) -> String` 신규 함수 추가 — 하나라도
  `disk_options.fencing == "resource-only"`인 리소스가 있으면 위 handlers
  블록을 포함한 파일 내용 생성 (노드당 파일은 하나뿐이므로 여러 리소스가
  있어도 블록은 한 번만). `global { usage-count yes; udev-always-use-vnr;
  }` 헤더도 실제 템플릿처럼 함께 포함.
- `generate_ansible_playbook()`에 "`/etc/drbd.d/global_common.conf` 배포"
  태스크 추가 (R09 순서 그대로: 커널 모듈 로드(8~9단계) 직후, DRBD 리소스
  up 이전) + `notify`로 `drbdadm adjust all` 핸들러 연결 (backup: yes도
  R09 패턴에 포함되어 있으므로 함께 반영).
- `/drbd/` 화면에 "fencing=resource-only 선택 시 global_common.conf가
  자동 포함됩니다" 안내 문구 추가 (선택 사항이지만 가이드 3.2절이 "겉보기엔
  문제없어 보인다"고 경고하는 함정이므로 UI 안내로 방지 가치가 큼).

### A.2 DRBD promotable clone — op timeout 세트가 monitor만 있고 나머지가 없음 + `clone-max` 하드코딩 버그

**✅ 구현 완료 (2026-09-04)** — `DrbdPacemakerResource`에
`promote_timeout`/`demote_timeout`/`start_timeout`/`stop_timeout` 필드
추가(기본값 90s/90s/240s/100s), `generate_drbd_resource_cmds()`가
demote/monitor(Promoted+Unpromoted)/notify/promote/reload/start/stop
op를 전부 출력하도록 수정. `clone-max`는 `config.cluster.nodes.len()`
기준으로 동적 계산(노드 목록이 비어 있으면 3으로 폴백)하도록
`generate_pcs_script()`/`generate_drbd_resource_cmds()` 시그니처 변경.
`routes/pacemaker.rs`의 `DrbdGroupInput`과 `templates/pacemaker/index.html`
DRBD+FS 카드에 4개 op timeout 입력 필드 추가(예전 형식 localStorage
데이터도 기본값으로 안전하게 채움). 단위 테스트 4개 통과
(`cargo test pacemaker::`, 3-node 기준 clone-max=3 확인 포함).

- 가이드 1.1절 필수 세트: `op demote/monitor/notify/promote/reload/start/stop`
  전부 명시, 특히 promote/demote 90~240s.
- 현재 `generate_drbd_resource_cmds()` (`src/generators/pacemaker.rs:104-`)는
  `op monitor interval=29s role=Promoted` / `interval=31s role=Unpromoted`
  **두 줄만** 생성하고, start/stop/promote/demote/notify/reload op는 전혀
  지정하지 않는다 (pcs/pacemaker 기본값에 의존하게 됨 — 가이드 3.5절이
  경고하는 "너무 짧은 기본 timeout → failcount=INFINITY 연쇄"의 위험 그대로
  노출).
- **`R11.pacemaker_drbd_resources.yml`에서 실제 운영 중인 6개 DRBD clone의
  op 세트를 그대로 확인함** — 리소스마다 살짝 다르지만 공통 프리셋이
  보인다 (기본값 후보로 그대로 채택 가능):
  | op | 기본값 | 크거나 느린 리소스(예: bdii_config) |
  |---|---|---|
  | `demote` | `timeout=90` | `timeout=120s` |
  | `monitor` | `interval=20s` (역할 구분 없음) | 동일 (단, `drbd_cecm_varlib`처럼 `role=Promoted interval=10 timeout=20` / `role=Unpromoted interval=20 timeout=20`로 나누는 예도 있음) |
  | `notify` | `interval=0s timeout=90` | 동일 |
  | `promote` | `timeout=90` | `timeout=120s` |
  | `reload` | `interval=0s timeout=30` | 동일 |
  | `start` | `timeout=240` | 동일 (전 리소스 공통) |
  | `stop` | `timeout=100` | `timeout=180s` |

  현재 앱의 `29s`/`31s` monitor 값은 이 실측 프리셋과 근접하지만 임의값이므로,
  실제 운영값(`interval=20s`, Promoted/Unpromoted 분리는 선택)으로 맞추는
  것을 권장.
- **별개의 실제 버그**: `generate_drbd_resource_cmds()`는
  `clone-max=2 clone-node-max=1`을 **하드코딩**한다
  (`src/generators/pacemaker.rs`, `promotable` 블록). 그런데 이 앱의
  DRBD 폼 자체가 README.md 기준 "2–7 nodes"를 지원한다고 명시하고
  (`/drbd/` 화면, `DrbdResource.nodes`는 가변 길이), `R11.pacemaker_drbd_resources.yml`의
  실제 운영 클러스터도 3노드라 `clone-max=3`을 쓴다. **DRBD 리소스를
  3노드 이상으로 구성한 뒤 Pacemaker 탭에서 clone을 생성하면, 노드 수와
  무관하게 항상 `clone-max=2`가 나와 세 번째 이상 노드에는 DRBD clone이
  아예 배치되지 못하는 실제 버그**다.
- **결정(2026-09) — 이 앱의 기본/공식 대상은 3-node 클러스터다.** 2노드
  구성(쿼럼 tie-breaker/쿼럼 디바이스 등 별도 고려사항이 필요해 더
  어려움)은 이번 계획에서 다루지 않는다 — 지원 범위에서 명시적으로
  제외. 이걸로 README.md("3-nodes")와 `CLAUDE.md`("2-node") 간 표기
  불일치도 정리된다: **`CLAUDE.md`가 낡은 표기**이므로 이후 "3-node"로
  맞추는 것을 권장하되, 이 문서 자체를 고치는 건 별도 작업으로 남겨둔다
  (PLAN.md는 기능 코드 계획 문서이므로). `clone-max` 버그는 이 결정으로
  더 명확해진다 — **이 앱의 1순위 타깃인 3노드 구성조차 지금 하드코딩된
  `clone-max=2`로는 정상 동작하지 않는다.**
- **변경 대상**:
  - `DrbdPacemakerResource`(`src/models/pacemaker.rs:34-57`)에
    `promote_timeout`/`demote_timeout`/`start_timeout`/`stop_timeout`
    필드 추가 (기본값은 위 표 기준), `generate_drbd_resource_cmds()`에서
    `op demote/notify/promote/reload/start/stop` 라인을 전부 출력하도록
    수정.
  - `clone-max`를 하드코딩 대신 **클러스터 노드 수**(`PacemakerConfig.cluster.nodes.len()`,
    이미 존재하는 필드)로 계산해서 채우도록 `generate_drbd_resource_cmds()`
    시그니처 변경 (또는 호출부에서 노드 수를 함께 전달).
  - UI(Pacemaker 아코디언의 DRBD+FS 섹션)에 op timeout 필드 추가.

### A.3 생성된 pcs 스크립트에 멱등성 가드가 없음

**✅ 구현 완료 (2026-09-07)** — `src/routes/pacemaker.rs::build_resources_play()`가
`pcs resource config`/`pcs constraint --full`을 각각 딱 한 번만
`run_once`+`delegate_to: "{{ ansible_play_batch | first }}"`로 캡처한 뒤,
`push_guarded_resource_task()`/`push_guarded_constraint_task()`가 리소스/
Order/Colocation/Location 생성 태스크마다 `when: existing_*.stdout is not
search(...)` 가드를 붙인다. 사용자 입력(리소스/노드/클러스터 이름 등)에
`'`나 `"`가 섞여도 YAML이 깨지지 않도록 `yaml_squote()`/`yaml_dquote()`
헬퍼로 이스케이프(라이브 서버에 `cluster_name=ha"cluster`,
`ipmi_password=it's"secret` 같은 값을 실제로 POST해서 생성된 플레이북을
`python3 -c "import yaml; yaml.safe_load(...)"`로 파싱 검증). 단위 테스트
`routes::pacemaker::tests::resources_play_precaptures_state_and_guards_each_item`,
`order_and_colocation_constraints_are_individually_guarded`.

- 가이드 6.3절이 요구하는 멱등성을, `R11.pacemaker_drbd_resources.yml`이
  **실제로 어떻게 구현했는지 정확한 패턴을 확인함** — 리소스마다 개별로
  `pcs resource config <id>`를 호출하는 게 **아니라**:
  1. 맨 앞에서 딱 두 번만 상태를 통째로 캡처 (`pcs resource config` →
     `existing_resource_config`, `pcs constraint --full` →
     `existing_constraints`)
  2. 이후 모든 생성 태스크는 이 캡처된 텍스트에 대해
     `when: existing_resource_config.stdout is not search('Resource: ' ~
     item.id ~ ' \\(')`  (리소스), `when: existing_constraints.stdout is
     not search(item.source_role ~ " resource '" ~ item.source ~ "' with
     Promoted resource '" ~ item.target ~ "'")` (colocation),
     `... is not search(item.first_action ~ " resource '" ~ item.first ~
     "' then " ~ item.then_action ~ " resource '" ~ item.then ~ "'")`
     (order) 형태로 grep.
  이 방식은 리소스 N개당 pcs 호출 1번(총 2번)만 하므로, 리소스마다
  `pcs resource config <id>`를 부르는 방식보다 훨씬 빠르고 실제 운영에서
  검증된 패턴이다.
- **결론: 이 멱등성은 순수 bash pcs 스크립트보다 Ansible 플레이북
  래퍼 레벨에서 구현하는 게 실제 운영 방식과 일치**한다. 현재
  `generate_pcs_script()`(`src/generators/pacemaker.rs:6-`)가 만드는 순수
  bash 스크립트는 참고/수동실행용으로 남겨두되(멱등성 보장 없음을 스크립트
  상단 주석에 명시), **Ansible 배포 경로(`/pacemaker/generate`가 만드는
  Ansible 래퍼, `src/routes/pacemaker.rs`)를 R11과 동일한 구조로
  다시 만드는 것**을 권장.
- **변경 대상**: `src/routes/pacemaker.rs`의 Ansible 플레이북 생성 부분에
  `pcs resource config`/`pcs constraint --full` 사전 조회 태스크 2개를
  추가하고, 이후 리소스/제약 생성 태스크마다 위 `is not search(...)` 조건을
  붙이도록 재작성. `pcs_controller: "{{ ansible_play_batch | first }}"` +
  `delegate_to`/`run_once: true` 패턴도 그대로 채택 (모든 노드에서 반복
  실행하면 안 되고 대표 노드 1곳에서만 pcs 명령을 실행해야 함 — 현재
  래퍼가 이 패턴을 쓰고 있는지 구현 시작 시 먼저 확인).

### A.4 클러스터 부트스트랩(설치~cluster setup)이 생성되지 않음

**✅ 구현 완료 (2026-09-07)** — `src/routes/pacemaker.rs::build_bootstrap_play()`가
`cluster.nodes`가 비어 있지 않을 때만 1~7단계(저장소 활성화 → 패키지
설치 → pcsd 시작/활성화 → hacluster 계정 생성(uid 189, no_log) → `pcs
status` 사전 체크 → `pcs_status.rc != 0`일 때만 host auth/cluster
setup/start/enable → 속성 설정)를 새 play로 생성. 대표 노드 선택은
`groups['container']` 같은 리터럴 그룹명이 아니라 A.3과 동일하게 `{{
ansible_play_batch | first }}` / `{{ ansible_play_batch | join(' ') }}`로
통일(단위 테스트로 `groups['container']` 부재 확인). `hacluster_password`는
`vars_prompt`(`private: yes`)로 받아 파일에 평문이 남지 않음. 참고용
`generate_pcs_script()`는 그대로 두되 최상단에 "이 스크립트는 클러스터가
이미 있다고 가정하며 멱등성이 없다" 주석 추가.

- `generate_pcs_script()`는 `pcs property set stonith-enabled=...`부터
  시작 — **클러스터가 이미 만들어져 있다고 가정**한다.
- **`R02.service_pacemaker_setup.yml`(container 클러스터)과
  `R54.se_backend_pacemaker.yml`(se_backend 클러스터) — 서로 다른 두
  클러스터에 완전히 동일한 부트스트랩 절차**가 쓰이고 있어 표준 순서로
  확정할 수 있다:
  1. 저장소 활성화: `epel-release` 설치 + `dnf config-manager
     --set-enabled crb` + `--set-enabled highavailability` (AlmaLinux9
     기준, DRBD용 elrepo와는 별개 저장소)
  2. 패키지 설치: `pacemaker`, `corosync`, `pcs`, `fence-agents-all`
  3. `pcsd` 서비스 시작 + 활성화
  4. `hacluster` 계정 생성 — **uid 189 고정**, `shell=/sbin/nologin`,
     `home=/var/lib/hacluster`, `group=haclient`, 비밀번호는 vault에서
     읽은 평문을 `password_hash('sha512')`로 해시해서 전달 (`no_log:
     true` 필수 — 두 태스크 모두)
  5. **멱등성 체크**: `pcs status`를 대표 노드에서만 `run_once: true` +
     `delegate_to`로 실행, `failed_when: false`로 rc만 확인
  6. `pcs_status.rc != 0`일 때만 (즉 클러스터가 아직 없을 때만) 아래를
     **전부 대표 노드에서 `run_once`+`delegate_to`로 실행** (각 노드에서
     반복 실행 금지):
     `pcs host auth <전체 노드 목록> -u hacluster -p <비밀번호>` →
     `pcs cluster setup <cluster_name> <전체 노드 목록>` →
     `pcs cluster start --all` → `pcs cluster enable --all`
  7. `pcs property set no-quorum-policy=...` / `stonith-enabled=...`
     (이 앱이 이미 만드는 부분과 동일)

  > **주의(5~6단계를 원본 R02와 다르게 가져올 부분)**: R02 원본은
  > `pcs_master: "{{ groups['container'][0] }}"` / `pcs_nodes:
  > "{{ groups['container'] | join(' ') }}"`처럼 **리터럴 인벤토리
  > 그룹명 `container`**에 의존한다. 하지만 이 앱의 Ansible 래퍼는
  > (Quadlet/Pacemaker 기존 코드처럼) 사용자가 자유 입력하는
  > `ansible_hosts` 필드(기본값 `"all"`)로 대상 그룹을 정하므로 그룹명이
  > `container`라고 가정할 수 없다. 또한 A.3에서 이미 대표 노드 선택에
  > `ansible_play_batch | first`를 채택했으므로, 같은 파일
  > (`src/routes/pacemaker.rs`)을 고치면서 두 가지 다른 관용구를 섞으면
  > 안 된다. **`groups['container']` 대신 A.3과 동일하게 `pcs_master:
  > "{{ ansible_play_batch | first }}"` / `pcs_nodes: "{{
  > ansible_play_batch | join(' ') }}"`로 통일한다** (play의 `hosts:`
  > 값이 무엇이든 그 play가 실제로 대상으로 삼은 호스트 목록을 그대로
  > 참조하므로 그룹명 의존성이 없어짐).
- `ClusterConfig`(`src/models/pacemaker.rs:4-25`)에 이미 `cluster_name`,
  `nodes: Vec<ClusterNode>` 필드가 있음 — **모델은 준비돼 있고 생성기만
  이를 안 쓰고 있음**. `/nodes/` 탭에 이미 등록된 노드 목록(`db::get_nodes()`)을
  재사용해 채울 수 있음.
- **결정(2026-09) — `no_quorum_policy` 기본값은 `"stop"`을 유지한다**:
  현재 `ClusterConfig::default()`의 `no_quorum_policy` 기본값은
  `"stop"`이다. 실제 운영 클러스터(R02의 container 클러스터, R54의
  se_backend 클러스터)는 `no-quorum-policy=ignore`를 쓰지만, 이를
  "검증된 모범사례"로 보고 따라가지 않기로 했다 — **이 앱은 3노드를
  기본 대상으로 삼고(A.2 참고), 과반수 쿼럼을 구성하지 못하는 파티션은
  `stop`으로 리소스를 내리는 것이 맞다**는 결정. 근거:
  1. `stop`은 Pacemaker 자체의 업스트림 기본값이자 split-brain 방지의
     정석이다.
  2. 소수파 노드는 이미 DRBD의 `on-no-quorum io-error`(가이드 3.8절/4.2절)로
     디스크 I/O가 막힌다. 이 상태에서 pacemaker가 `ignore`로 리소스를
     계속 띄우려 하면 "I/O는 막혔는데 서비스는 뜨려는" hang 상태가 되고,
     `stop`이면 pacemaker도 같이 깔끔하게 내려가 이중 보호가 일관된다.
  3. (2노드 구성은 A.2 결정대로 이번 범위에서 다루지 않으므로, `stop`이
     2노드의 특수 쿼럼 처리와 상충하는지는 이 문서에서 고려하지 않는다.)

  결론: **코드 변경 없음** (현재 기본값 유지). A.4 구현 시 부트스트랩
  플레이북의 7단계에도 `no-quorum-policy=stop`을 그대로 반영한다.
- **변경 대상**: 이 부트스트랩은 (A.3과 마찬가지 이유로) 순수 bash
  스크립트보다 **Ansible 플레이북 쪽에 `pcs_status.rc != 0` 가드와 함께
  추가하는 것이 실제 패턴과 일치**한다. `src/routes/pacemaker.rs`의
  Ansible 래퍼 생성 부분에 위 1~7단계를 새 play로 prepend (`cluster.nodes`가
  비어 있으면 생략 — 기존 사용자 흐름과 하위 호환). 참고용 순수
  `generate_pcs_script()`에도 7단계(속성 설정)는 그대로 두되, 1~6단계는
  "직접 실행 시 아래 명령을 대표 노드 1곳에서만 실행하세요"라는 주석과
  함께 참고 명령으로만 추가 (스크립트 자체 멱등성은 A.3에서 다룸).
  `hacluster` 비밀번호는 평문 하드코딩 대신 Ansible Vault 참조 또는 실행
  시 프롬프트로 처리.

### A.5 STONITH 리소스 생성 + versionlock 안전장치 누락

**✅ 구현 완료 (2026-09-07)** — `models::pacemaker::StonithDevice { node,
ipmi_ip, ipmi_user, ipmi_password, extra_opts }` 추가, `PacemakerConfig`에
`stonith_devices: Vec<StonithDevice>` 필드 추가. `generate_pcs_script()`에
참고용 STONITH 블록 추가(`sanitize_stonith_id()`로 `ip=`/`user=`/`password=`
파라미터 키 사용, `[^A-Za-z0-9._-]→_` 이름 정규화). 실제 배포는
`build_stonith_play()`가 STONITH 장치가 하나 이상 있을 때만 별도 play로
생성 — `pcs stonith status`를 한 번 캡처해서 `stonith-ipmi-<node>` 문자열
존재 여부로 가드. `build_bootstrap_play()`의 패키지 설치 직후 단계에
`dnf-plugin-versionlock` 설치 + `dnf versionlock add pacemaker corosync
pcs`를 즉시 추가(유지보수 시에만이 아니라 부트스트랩의 일부). 입력 폼
UI(`templates/pacemaker/index.html`)에 "STONITH(펜싱) 장치" 아코디언
섹션 추가 — 노드별 IPMI IP/사용자/비밀번호/추가옵션 입력, 선택된 클러스터
노드 목록과 연동되는 드롭다운.

- `stonith_enabled` 불리언만 있고 실제 STONITH 리소스 생성 명령이 없음.
  **`R02.service_pacemaker_stonith.yml`과 `R55.se_backend_stonith.yml`
  (두 클러스터에 동일 패턴)에서 정확한 명령 형태를 확인함**:
  ```
  pcs stonith create stonith-ipmi-<노드명 정규화> fence_ipmilan \
    pcmk_host_list=<노드명> \
    ip=<IPMI IP> user=<IPMI user> password=<IPMI password> \
    lanplus=1 power_wait=5 pcmk_reboot_timeout=300 \
    pcmk_monitor_timeout=60 pcmk_reboot_action=reboot
  ```
  주의: 파라미터 키는 `ip=`이지 `ipaddr=`가 아니다(이전 초안의 오기 수정).
  리소스 이름은 `stonith-ipmi-<호스트명을 [^A-Za-z0-9._-]→_로 치환>` 규칙.
  멱등성은 `pcs stonith status` 결과를 미리 캡처해서 해당 리소스 이름
  문자열이 있는지로 판단(패턴은 A.3과 동일한 사전-캡처 방식).
  두 노드(IPMI 정보/계정 없는 노드)는 `when: hostvars[item].ipmi_ip is
  defined and ipmi_user is defined and ipmi_pass is defined`로 자동
  skip — 즉 IPMI 정보가 없는 노드가 섞여 있어도 안전.
- 가이드 3.6절/5.2절 + **`R02.service_pacemaker_setup.yml` 실제 코드**:
  `pacemaker`/`corosync`/`pcs` versionlock은 "유지보수 시에만" 거는 게
  아니라, **부트스트랩(A.4) 직후 설치 스텝의 일부로 즉시 건다** — R02는
  `dnf-plugin-versionlock` 설치 → 패키지 설치 직후 바로
  `dnf versionlock add pacemaker corosync pcs`를 실행한다. 잠금 해제는
  `R56.cluster_rolling_maintenance.yml`(A.7)에서만 한다.
- **변경 대상**:
  - `models/pacemaker.rs`에 `StonithDevice { node, ipmi_ip, ipmi_user,
    ipmi_password, extra_opts: Vec<String> }` 목록 추가 (파라미터 키
    `ip=`/`user=`/`password=`로 통일), 참고용 `generate_pcs_script()`에
    STONITH 블록 추가 (비어 있으면 생략). 실제 멱등 배포는 A.3/A.4처럼
    Ansible 래퍼 쪽에 `pcs stonith status` 사전 캡처 + `is not search(...)`
    가드로 구현.
  - A.4의 부트스트랩 Ansible 플레이북에 `dnf-plugin-versionlock` 설치 +
    `dnf versionlock add pacemaker corosync pcs` 태스크를 **패키지 설치
    직후**(별도 유지보수 단계가 아니라) 추가.

### A.6 서로 다른 DRBD clone 간 anchor/follower 결합이 모델링돼 있지 않음

**✅ 구현 완료 (2026-09-07)** — 코드 변경 없이(모델은 이미 범용
`ColocationConstraint`로 표현 가능하므로) `templates/pacemaker/index.html`의
제약조건 아코디언에 "DRBD-DRBD anchor/follower 결합 빠른 추가" 위젯 추가:
Anchor Clone/Follower Clone 두 드롭다운(등록된 DRBD 볼륨의 `clone_name`
목록에서 자동 채워짐, `refreshCloneSelectors()`)과 "추가" 버튼이
`rsc=follower, rsc_role=Promoted, with_rsc=anchor, with_rsc_role=Promoted,
score=INFINITY` 순서로 고정된 `ColocationConstraint`를
`colocation_constraints_json` 에디터에 append(`addAnchorFollowerConstraint()`).
화면에 "이동시킬 땐 anchor 쪽을 옮기세요" 안내 문구 포함.

- 가이드 2.2절/6.4절: 두 DRBD clone을 "항상 같은 노드에서 Promoted"로
  묶을 때 `Promoted A with Promoted B`는 **방향이 있다** (B=anchor,
  A=follower). anchor를 몰라도 되는 걸로 취급하면, 실제 운영에서
  follower만 이동시키려다 promotion score가 -INFINITY로 막히는 사고가
  난다 (가이드 7.2절 실사고).
- 현재 `ColocationConstraint`(`src/models/pacemaker.rs:117-124`)는 범용
  구조라 이 관계를 표현할 수는 있지만, "어느 쪽이 anchor인지"를 UI/모델
  차원에서 강제하거나 안내하지 않는다.
- **변경 대상**: 필수는 아니지만 권장 — Pacemaker UI의 제약조건 섹션에서
  "DRBD-DRBD 결합" 전용 입력을 추가할 때 `anchor_clone`/`follower_clone`
  두 필드로 받아서 `rsc=follower, with_rsc=anchor` 순서를 코드가 강제
  생성하도록 하고, 화면에 "follower가 anchor를 따라갑니다. 이동시킬 땐
  anchor(●) 쪽을 옮기세요" 같은 안내를 붙인다.

### A.7 롤링 유지보수(standby) 스크립트 생성 없음 — 선택 기능

**✅ 구현 완료 (2026-09-07)** — `generate_maintenance_script(config:
&PacemakerConfig) -> String`(`src/generators/pacemaker.rs`) 신규 생성기.
`cluster.nodes`가 비어 있으면 빈 문자열 반환. 가이드 5.1절 13단계를
노드별 bash 루프로 구현(대표 노드로 SSH해서 `pcs node standby`/
`unstandby`, 각 노드로 직접 SSH해서 versionlock 해제/패치, `pcs cluster
stop`/`start`, `dnf update -y`, 재부팅+대기). standby 직후/최종 확인 두
지점은 `read -r -p` 사람 확인 대기(무인 자동화 아님 — STONITH 연쇄
오탐 위험 때문에 의도적 설계). 가이드 8.4절의 알려진 공백(비-Pacemaker
`Restart=always` Quadlet 컨테이너는 이 절차로 못 건드림) 주석 포함.
결과 화면(`templates/pacemaker/result.html`)에 "pacemaker_maintenance.sh"
다운로드/복사 카드 추가.

- 가이드 5.1절 절차 (versionlock 해제 → standby → 리소스 이동 확인 →
  `pcs cluster stop` → 업데이트 → 재부팅 → `pcs cluster start` →
  unstandby → 확인, 실패 시 다음 노드로 넘어가지 않음) + parent repo
  `R56.cluster_rolling_maintenance.yml`.
- **변경 대상**: `generate_maintenance_script(config: &PacemakerConfig) ->
  String` 신규 생성기 함수 — `cluster.nodes` 목록만 있으면 만들 수 있음.
  Pacemaker 결과 화면에 "유지보수 스크립트 다운로드" 버튼 추가.
- 가이드 8.4절이 명시한 기존 알려진 공백(비-Pacemaker, `Restart=always`
  Quadlet 컨테이너는 이 절차가 못 건드림 — 별도 `systemctl stop` 필요)도
  생성된 스크립트 주석에 그대로 반영해서 사용자가 인지하게 할 것.

### A.8 Ansible 인벤토리 그룹 존재 검증 태스크 없음 — 선택 기능

**✅ 구현 완료 (2026-09-07)** — `generate_pacemaker_ansible_playbook()`이
생성하는 플레이북 최상단에 `ansible-inventory -i <inventory> --graph`로
`hosts:` 값이 예상 노드 수만큼 resolve되는지 실행 전 확인하라는 주석
추가(주석/문서화 수준 — 가이드가 요구하는 최소 반영).

- 가이드 3.4절/6.5절: `hosts: <group>`이 인벤토리에 없으면 ansible이
  에러 없이 대상 0개로 조용히 끝난다 (실사고 7.3절).
- **변경 대상**: `src/routes/pacemaker.rs`가 만드는 Ansible 플레이북
  래퍼(예: 305행 부근 `hosts: {hosts}`) 앞에, 별도 `pre_tasks` 또는
  로컬 `ansible-inventory --graph` 사전 체크 안내 주석을 추가. (앱이
  로컬에서 인벤토리를 직접 만들기 때문에 이 위험은 실제로는 낮지만,
  가이드가 명시적으로 요구하는 항목이라 최소 주석/문서화는 반영.)

### A.9 클러스터 방화벽 포트(corosync/pcsd)를 생성된 Ansible에서 열지 않음

**✅ 구현 완료 (2026-09-07)** — `build_bootstrap_play()`의 패키지 설치
단계에 `ansible.posix.firewalld` 태스크로 `5404-5405/udp`(corosync),
`2224/tcp`(pcsd)를 work zone에 permanent+enabled로 여는 태스크 추가
(`src/generators/drbd.rs`의 DRBD 포트 오픈 태스크와 동일한 패턴).

- **`R01.service_pacemaker_firewall.yml`(container 클러스터)과
  `R53.firewalld_pacemaker.yml`(se_backend 클러스터) — 역시 두 클러스터
  모두 동일하게** `firewalld` work zone에 다음을 연다:
  `5404-5405/udp`(corosync), `2224/tcp`(pcsd), DRBD 포트(예 `7788/tcp`).
  (`1229/tcp`도 두 파일 모두 열려 있으나 용도가 사이트별 부가 서비스로
  보이며 pacemaker/corosync/pcsd 자체와 직접 관련은 없어 이식 대상에서
  제외 — 필요해지면 이후 확인.)
- **현재 `src/routes/pacemaker.rs`의 Ansible 래퍼는 pacemaker/corosync
  패키지를 설치만 하고, corosync(5404-5405/udp)나 pcsd(2224/tcp) 포트를
  여는 firewalld 태스크가 전혀 없다** (grep으로 `firewalld`/`5404`/`2224`
  전부 미검출 확인함). DRBD 쪽은 `generate_ansible_playbook()`
  (`src/generators/drbd.rs`)이 DRBD 포트 range는 열지만 corosync/pcsd는
  다루지 않는다 — 둘 다 부분적으로만 방화벽을 열고 있는 셈.
  (README.md/README.kor.md의 사전준비 방화벽 안내에도 `2224/tcp`가
  빠져 있었음 — 이번에 직접 수정함.)
- **변경 대상**: A.4의 부트스트랩 Ansible 플레이북(패키지 설치 단계)에
  `ansible.posix.firewalld` 태스크로 `5404-5405/udp`, `2224/tcp`를 work
  zone에 permanent+enabled로 추가 (기존 `generate_ansible_playbook()`의
  DRBD 포트 오픈 태스크, `src/generators/drbd.rs`의 "10. 방화벽 포트 허용"
  부분과 동일한 `ansible.posix.firewalld` 패턴 재사용).

---

## B. nftables 호스트 필터 — 이 앱의 유일한 방화벽 제어 지점

**사용자 결정 (2026-09)**: **pod/컨테이너 트래픽**에 대한 방화벽 제어는
**호스트 레벨 nft netdev table 필터(이 섹션) 하나로 일원화**한다. Pod
내부 네트워크 네임스페이스에서 도는 2차 방어선(사이드카 방화벽)은
**구현하지 않는다** — 이전 초안에 있던 별도 섹션이었으나 범위에서
제외했다 (자세한 이유는 "범위에서 제외한 것" 참고).

(범위 구분: 이 섹션은 **컨테이너/pod 목적지 트래픽**만 다룬다. corosync/
pcsd 같은 **클러스터 인프라 자체의 포트**를 여는 것은 별개 계층
[firewalld, A.9]이며 이 "일원화" 결정과 충돌하지 않는다 — 실제
`ipvlan_l2.nft`도 매칭되지 않는 트래픽은 마지막에 `accept`로 흘려보내
호스트 자체 통신/firewalld 처리로 넘긴다.)

### B.1 established/DNS 응답 우회 규칙 누락

**✅ 구현 완료 (2026-09-07)** — `generate_nft_policy()`의 체인 헤더 출력
직후에 `tcp flags & (ack | rst) != 0 accept`/`udp sport 53 accept` 두 줄을
항상 추가하도록 수정. 단위 테스트로 두 줄이 체인 헤더 바로 뒤에 오는지
확인, `/nft/generate` 실제 호출로도 확인.

실제 운영 중인 `~/playbooks_cmst2/container_playbooks/files/nft/ipvlan_l2.nft`
(netdev ingress, `policy accept`)는 체인 맨 앞에

```
tcp flags & (ack | rst) != 0 accept
udp sport 53 accept
```

를 둔다 — 컨테이너가 먼저 시작한 아웃바운드 연결의 응답 패킷을 통과시키기
위함. 현재 `generate_nft_policy()` (`src/generators/nft.rs:118-166`)는 이
두 줄이 없다. 이 상태로는 컨테이너의 아웃바운드 TCP/DNS 응답이 드롭될 수
있다.

**재사용**: `NftPolicy`는 그대로, 체인 헤더 출력 직후(`nft.rs:122` 부근)에
두 줄만 추가하면 됨. 새 모델 필드 불필요 (항상 켜는 것을 권장 — 실제
운영 파일과 동일 동작이 되고 코드 변경도 최소화됨).

### B.2 [신규] 생성된 `.nft` 파일을 클러스터 전 노드에 동일하게 배포하는 Ansible이 없음

**✅ 구현 완료 (2026-09-07)** — `generate_nft_ansible_playbook(policy,
ansible_hosts, ansible_user, ansible_ssh_key)`를 `src/generators/nft.rs`에
추가 (Quadlet/Pacemaker와 동일한 자유입력 `ansible_hosts` 관례, 기본값
"all"). R99 패턴 그대로 `/etc/nftables/ipvlan_l2.nft` 배포(`backup: yes`)
+ `/etc/sysconfig/nftables.conf` include 보장 + nftables 재시작 핸들러.
`/nft/` 폼에 Pacemaker와 동일한 Ansible 프로파일 선택 UI 추가,
`result.html`의 기존 **하드코딩된 정적 Ansible 스니펫**(`hosts: all` 고정,
R99와 다른 경로)을 실제 생성기 결과로 교체. 단위 테스트 3개 통과, 실제
서버로 `/nft/generate` end-to-end 호출해서 `ansible_hosts` 반영과
R99 경로 정확성까지 확인.

- 서비스(pod)는 DRBD/Pacemaker failover로 클러스터의 임의 노드로 옮겨갈
  수 있다. 목적지 IP 기준 allow 규칙(`target_<name>_v4/v6`)은 **pod가
  어느 노드에서 뜨든 그 노드의 nft 필터에 이미 존재**해야 한다 — 한
  노드에만 규칙이 있으면 failover 직후 새로 옮겨간 노드가 해당 서비스
  트래픽을 드롭해버린다. (이번에 사용자가 명시적으로 짚은 요구사항.)
- 실제 `R99.nftables.yml`이 정확히 이 요구사항을 구현한다: `hosts:
  container_service`(개별 노드가 아니라 **클러스터 전체 노드 그룹**)에
  대해 동일한 `files/nft/ipvlan_l2.nft`를 `copy: backup: yes`로 배포하고,
  `/etc/sysconfig/nftables.conf`에 `include "..."` 줄을 `lineinfile`로
  보장한 뒤 `notify: restart nftables service`.
- **현재 상태**: `src/routes/nft.rs::generate()`는 `.nft` 파일 내용을
  만들어 결과 화면에 보여줄 뿐, DRBD(`generate_ansible_playbook()`,
  `src/generators/drbd.rs`)/Pacemaker/Quadlet과 달리 **배포용 Ansible
  플레이북을 전혀 생성하지 않는다** — 전체 노드 배포를 사용자가 수동으로
  복붙해야 해서, 한 노드만 갱신하고 나머지를 빠뜨리기 쉬운 지점이다.
- **변경 대상**: `src/generators/nft.rs`(또는 신규 모듈)에 R99 패턴을
  이식하는 함수를 추가한다. **주의**: 초안에서는
  `generate_nft_ansible_playbook(policy: &NftPolicy, nodes: &[DbNode])`처럼
  DB 노드 목록을 직접 받는 시그니처를 제안했었는데, 확인해보니 이 앱의
  기존 Ansible 생성기 관례와 맞지 않는다 — `generate_quadlet_ansible_playbook()`
  (`src/routes/quadlet.rs:227-`)와 Pacemaker 쪽 래퍼는 둘 다
  `ansible_hosts: Option<String>`(기본값 `"all"`) **자유 입력 문자열**을
  받아 `hosts: {hosts}`로 그대로 꽂아 넣는 패턴이다 (DRBD만 예외적으로
  `AnsibleInventory`에 노드별 IP를 직접 담는데, 이건 `.res` 파일 자체가
  노드별 복제 IP를 필요로 해서다 — nft 배포에는 그런 이유가 없음). 따라서
  nft도 **Quadlet/Pacemaker와 동일하게** `ansible_hosts`/`ansible_user`/
  `ansible_ssh_key` 폼 필드를 받는 시그니처로 통일한다:
  `generate_nft_ansible_playbook(policy: &NftPolicy, ansible_hosts: &str,
  ansible_user: &str, ansible_ssh_key: &str) -> String`. 내용은 R99
  패턴 그대로: `copy` 태스크(`backup: yes`), `/etc/sysconfig/nftables.conf`에
  `lineinfile`로 include 보장, `notify: restart nftables` 핸들러.
  `/nft/` 폼에 다른 탭과 동일한 Ansible 입력 필드(호스트/유저/키) +
  결과 화면에 "Ansible 플레이북" 표시/다운로드 영역 추가.

---

## C. Quadlet Container 유닛 — `[Install]` / `AddCapability=` 누락

**✅ 구현 완료 (2026-09-07)** — `models::quadlet::QuadletContainer`에
`wanted_by: Option<String>`, `add_capabilities: Vec<String>` 필드 추가.
`generate_container_unit()`이 `AddCapability=`를 라벨/extra_args 근처에,
`[Install]\nWantedBy=...`를 `[Service]` 블록 뒤에 출력(`wanted_by`가
없으면 `[Install]` 섹션 자체를 생략). `templates/container/index.html`의
"AutoUpdate / 서비스 설정" 카드에 두 입력 필드 추가하고
`serializeContainerForm()`에 반영.

구현 중 **이 기능과 무관한 실제 버그를 하나 발견해 함께 수정함**:
`src/routes/container.rs::generate()`가 `containers_json`을 받을 때
`podman_inspect_to_quadlet()`을 **먼저** 시도하고 있었는데, 이 함수는
어떤 JSON 배열이 들어와도 (필드가 없으면 `"unknown"`/빈 문자열로
기본값을 채워) 절대 에러를 내지 않아서, 폼 기반 UI가 실제로 보내는
정상적인 `QuadletContainer` JSON 배열이 **매번** 빈 `unknown.container`로
잘못 해석되고 있었다 (라이브 서버에 실제 폼 데이터를 POST해서 재현
확인). `POST /container/from-inspect`(`from_inspect()`)가 이미 podman
inspect 원본 JSON을 다루는 전용 엔드포인트이므로, `generate()`는 이제
`containers_json`을 곧바로 `Vec<QuadletContainer>`로만 파싱하도록 수정—
재현 테스트로 `ContainerName=`/`AddCapability=`/`[Install]` 모두 정상
출력됨을 확인.

- `generate_container_unit()` (`src/generators/quadlet.rs:193-321`)은
  `[Install]` 섹션을 만들지 않는다 (`generate_volume_unit()`/
  `generate_bind_volume_unit()`은 이미 하드코딩돼 있음, quadlet.rs:33-34,
  187-188). 실제 플레이북의 pod-생명주기 종속 oneshot/사이드카 컨테이너
  (`bdii-nft-setup.container`, `condor-cm-nft.container`)는
  `[Install] WantedBy=<pod>.service` 또는 `WantedBy=multi-user.target`을
  명시한다.
- `condor-cm-nft.container`의 `AddCapability=NET_ADMIN`은 현재
  `extra_args`(→ `PodmanArgs=`)로는 네이티브하게 표현 불가
  (`PodmanArgs=AddCapability=NET_ADMIN`처럼 잘못 나옴; `--cap-add=NET_ADMIN`을
  extra_args에 넣는 우회는 가능).

**변경 대상**:
- `src/models/quadlet.rs:57-87` `QuadletContainer`에 `wanted_by:
  Option<String>`, `add_capabilities: Vec<String>` 필드 추가 (+ `Default`
  impl 89-115 갱신)
- `src/generators/quadlet.rs` `[Service]` 블록 뒤에 `[Install]` 출력,
  labels/extra_args 근처에 `AddCapability=` 출력 로직 추가
- `templates/container/index.html`의 `serializeContainerForm()`
  (416-478행)과 `cServiceType`/`cRemainAfterExit` 입력 자리(153-167행)
  패턴을 따라 UI 필드 추가

(참고: 이 필드들의 주 동기였던 "pod 내부 방화벽 사이드카"는 범위에서
제외되었으므로, 우선순위는 낮다 — Quadlet 스펙 완성도 차원의 선택적
개선으로 남겨둔다.)

---

## D. [신규 제안, 설계 스케치 단계] LINSTOR 백엔드 지원

**배경**: 현재 이 앱은 순수 DRBD 커널모듈(`drbdadm` + 수동 `.res` 파일)만
다룬다. 사용자가 LINSTOR(DRBD 위에서 스토리지 풀/리소스 그룹/노드 배치를
관리해주는 LINBIT의 오케스트레이션 레이어) 도입을 검토 중이며, **VM
테스트 클러스터도 LINSTOR로 구성할 예정**이라 우선순위가 낮지 않다.

**범위 확정 (사용자 결정, 2026-09)**:
1. **LINSTOR 소스 다운로드/컴파일/빌드/설치 자동화는 이 저장소에 넣지
   않는다.** LINBIT 공식 RPM 저장소는 구독/구매 고객 전용이라 소스 빌드가
   필요한데, 그 빌드 파이프라인은 **별도 코드 + 별도 GitHub Actions
   워크플로우**로 분리한다. 이 앱은 그 결과물(패키지 또는 설치
   스크립트/아티팩트)을 "소비"하는 입장만 맡는다.
2. **✅ 조사 완료(2026-09, 웹 검색 기준) — EL9(AlmaLinux9/RHEL9)용 무료
   공식 저장소는 존재하지 않는다.** LINBIT 문서/KB를 직접 확인한 결과:
   - `packages.linbit.com/public/`(진짜 무료 공개 저장소)는 **Debian/Proxmox
     전용**이고, Ubuntu는 별도 무료 PPA(`launchpad.net/~linbit/...`)가
     있다 — **둘 다 RHEL 계열이 아니다.**
   - EL9를 포함한 RHEL 계열은 DRBD 프리빌드 패키지와 LINSTOR
     (controller/satellite/client) 전부 **유료 구독 고객 계정(customer
     hash)이 있어야 접근되는 저장소**로만 배포된다 (DRBD 9 공식
     사용자 가이드: "커스터머 포털 계정이 필요").
   - "Staging" 저장소(릴리스 후보)도 동일한 customer hash 인증이
     필요해 무료가 아니다.
   - "평가판(1개월 무료 평가 등)"은 명시적으로 기간 제한이 있어 이번
     조사에서 제외 대상.
   - COPR/ELRepo 등 제3자 커뮤니티 저장소에도 LINSTOR RPM은 없는 것으로
     보인다 — ELRepo는 DRBD 커널모듈(`kmod-drbd9x`, 이미 R09/A.1이 쓰는
     방식)만 다루고, LINSTOR controller/satellite(Java/Rust 애플리케이션)는
     대상이 아니다.
   - 대신 **소스코드는 상시 무료 다운로드 가능**하고
     (`github.com/LINBIT/drbd`, `github.com/LINBIT/linstor-server`),
     LINBIT 스스로도 "release tarball에서 spec 파일로 표준 RPM 빌드
     방식이 그대로 동작한다"고 문서화해뒀다 — 즉 **소스 빌드(D.3)가
     LINBIT이 인정하는, 비구독자를 위한 유일한 경로**다.

   **결론: "무료 저장소 우선 사용" 조건은 폐기 — D.3(별도 빌드
   파이프라인)이 유일한 설치 경로로 확정.** 이 앱(D.1)은 처음부터 D.3의
   산출물(빌드된 RPM 또는 tarball+설치스크립트)을 소비하는 것으로
   설계한다.
3. **설치된 게 LINSTOR인지 순수 커널모듈인지는 "설치된 환경"을 보고
   판단**하고, 그 판단 결과를 설정 정보로 저장한다.
4. 그 판단(감지)은 **웹 UI(서버) 기동 시마다 갱신**한다.

### D.1 설치 환경 감지 + 설정 저장

- **재사용**: `src/scan.rs`의 기존 `startup_scan()` 패턴(서버 기동 시
  `tokio::spawn`으로 백그라운드 실행, `main.rs`에서 이미 호출 중)을 그대로
  확장 — DRBD/Quadlet/pcsd 스캔과 나란히 "스토리지 백엔드 감지" 단계
  추가.
- **감지 방법(제안)**: 노드별로 `linstor` CLI 존재 여부, 또는
  `linstor-controller`/`linstor-satellite` systemd 유닛 존재/활성 여부를
  확인. 로컬 스캔이 아니라 원격 노드 대상이므로, 기존
  `POST /api/collect-interfaces`가 쓰는 Ansible 실행 패턴(임시 인벤토리 +
  playbook 생성 → 실행 → 결과 파싱, `src/routes/nodes.rs`)을 재사용하는
  것이 자연스럽다.
- **저장 위치(제안)**: 노드별로 다를 수 있으므로 `nodes` 테이블에
  `storage_backend TEXT NOT NULL DEFAULT 'kernel-module'` 컬럼 추가
  (`'kernel-module' | 'linstor'`), 기존 `ALTER TABLE ... ADD COLUMN`
  마이그레이션 패턴(`src/db.rs`) 재사용. 클러스터 전체가 한 방식으로
  통일된다고 가정할 수 있으면 전역 설정 하나로 단순화해도 됨 — 실제
  LINSTOR 배포가 클러스터 내 노드마다 혼재 가능한지 여부에 따라 결정
  (일반적으로는 클러스터 전체가 한 방식으로 통일되므로 전역 설정 쪽을
  권장).
- `/nodes/` 화면에 감지된 백엔드를 배지로 표시.

### D.2 DRBD 모델/생성기의 LINSTOR 확장

**✅ 중요 발견(2026-09) — 직접 CLI 명령을 생성할 필요가 없다.** LINBIT이
공식 Ansible 컬렉션 `linbit.linstor`(+ 의존 컬렉션 `linbit.common`,
`linbit.drbd`, `linbit.drbd_reactor`)를 제공하며, `roles/`와
`plugins/modules/` 전체를 직접 열어 확인했다:

- **설치 role**: `controller_install`/`satellite_install`/`client_install`.
  실제 태스크(`tasks/main.yml`)를 확인한 결과 `ansible.builtin.package:
  name: linstor-controller` 같은 **일반 패키지 모듈만 쓰고 LINBIT 저장소
  URL을 하드코딩하지 않는다.** 이미 systemd 유닛이 설치돼 있으면
  (`ansible.builtin.stat`으로 확인) install 단계를 건너뛰고 설정
  단계로만 진행한다.
- **리소스 관리 모듈**: `resource`/`resource_definition`/`resource_group`/
  `storage_pool`/`node`/`snapshot`/`backup`/`key_value_store` 등 LINSTOR
  전 기능을 커버하는 선언적 모듈 세트가 이미 갖춰져 있다.
- 라이선스 MIT/GPL-3.0, 최근 업데이트(2026-09) — 활발히 유지보수됨.

**결론: 역할 분담이 깔끔하게 나뉜다.**
1. `gsdc-linbit-build`(D.3)가 만든 RPM을 대상 노드에 로컬 설치
   (`dnf install ./linstor-common-*.rpm ./linstor-controller-*.rpm ...`,
   같은 트랜잭션으로 묶어 의존성 자동 해결).
2. `linbit.linstor.controller_install`/`satellite_install`/`client_install`
   role을 그대로 실행 — 이미 설치된 상태를 감지하고 설정/방화벽/서비스
   기동만 이어서 처리한다.
3. 리소스(볼륨/스토리지풀) 생성·관리는 `linbit.linstor`의
   `resource_definition`/`resource_group`/`storage_pool` 등 모듈을
   Ansible 태스크로 조합해서 쓴다 — **이 앱이 직접 `linstor` CLI 문자열을
   조립할 필요가 없다.**

이전 초안에 있던 "신규 모델(`LinstorStoragePool` 등) + `linstor` CLI 명령을
직접 생성하는 생성기" 설계는 폐기한다 — 공식 컬렉션의 모듈을 그대로 쓰는
쪽이 훨씬 견고하고 유지보수 부담도 적다.

- **A.1과의 관계(확인됨)**: LINSTOR는 DRBD 커널모듈+`drbd-utils`를
  **대체하지 않고 그 위에서 오케스트레이션**한다. `linbit.drbd` 컬렉션도
  범위가 "DRBD 커널모듈/패키지 설치"(`drbd_install` role)뿐이라 fence-peer
  핸들러(A.1)와 겹치지 않는다 — A.1 작업은 LINSTOR 도입과 무관하게 계속
  유효하다. 다만 `.res` 파일 자체는 LINSTOR 새틀라이트가 자동 생성/관리
  하므로, 이 앱이 지금처럼 `/etc/drbd.d/*.res`를 직접 `copy`로 밀어넣는
  A.1의 방식과 실제로 충돌하는지는 **VM 테스트 클러스터에서 확인 필요**
  (LINSTOR가 관리하는 리소스에 한해서만 우회하면 될 가능성이 높음).
- **단계적 접근 제안**:
  - Phase 1 (D.1과 함께, 낮은 리스크): 감지 결과를 UI에 정보로만
    표시. 생성기 로직 변경 없음.
  - Phase 2: `linbit.linstor`/`linbit.common`/`linbit.drbd` 컬렉션을
    `ansible-galaxy collection install`로 설치하는 태스크 + 위 3단계
    (로컬 RPM 설치 → `*_install` role → 리소스 관리 모듈)를 조합하는
    Ansible 플레이북 생성기 추가. 기존 `DrbdResource`/`generate_res_file()`/
    `generate_ansible_playbook()`는 그대로 두고 **완전히 별도 경로**로
    만드는 것을 권장 — 기존 코드 회귀 위험 없이 점진적으로 확장 가능하고,
    A.1~A.9에서 이미 계획한 DRBD 관련 변경들과 충돌할 위험도 없다.
  - UI: `/drbd/` 탭에서 D.1의 감지 결과에 따라 "커널모듈 방식" 폼과
    "LINSTOR 방식" 폼을 분기 표시(탭 또는 라디오 선택).

### D.3 (이 저장소 범위 밖 — 참고용 메모) LINSTOR 소스 빌드 파이프라인

- 범위 확정 1번에 따라 **별도 저장소**(`gsdc-linbit-build`,
  `github.com/geonmo/gsdc-linbit-build`, private) + GitHub Actions
  워크플로우로 진행 중 — `linstor-server`(linstor-common/controller/
  satellite), `linstor-api-py`(python-linstor), `linstor-client`를
  각 저장소의 spec/Makefile/build.gradle을 직접 확인해 EL9용 RPM으로
  빌드하도록 구성했다 (`linstor-gui`는 controller가 서빙하는 선택적
  정적 대시보드로 판단해 제외). server와 client/api-py는 서로 독립된
  버전 체계를 쓰는 것도 확인해 반영함. 아직 실제 실행 검증 전.
- D.2에서 확인한 대로, 이 산출물(RPM)은 노드에 로컬 설치한 뒤
  `linbit.linstor` 공식 Ansible 컬렉션의 `*_install` role이 "이미 설치됨"을
  감지해 설정을 이어받는 방식으로 소비한다 — RPM 저장소 메타데이터
  (`createrepo_c`)까지는 필요 없고, `dnf install ./*.rpm` 로컬 설치로
  충분하다.

**결정 필요(사용자 확인 필요, 구현 착수 전)**:
- ~~LINBIT EL9 공개 저장소의 실제 포함 범위~~ → 조사 완료, 무료 저장소
  없음으로 확정 (위 범위 확정 2번 참고).
- 클러스터 내 노드별 백엔드 혼재를 허용할지, 전역 설정 하나로 충분한지.
- LINSTOR가 관리하는 `.res` 파일 경로/방식이 현재 A.1의 `global_common.conf`
  배포 방식과 실제로 충돌하는지 (VM 테스트 클러스터에서 확인 가능하면
  가장 확실함).
- D.3(별도 빌드 파이프라인)의 산출물 형태(RPM vs tarball+설치스크립트) —
  D.1의 설치 Ansible 태스크 설계에 직접 영향.

---

## 검증/테스트 계획

항목별로 "생성된 텍스트가 맞는가"와 "실제 클러스터에서 의도대로 동작하는가"
두 층위를 나눠서 검증한다. 이 앱은 파일 생성기이므로 1차는 `cargo test`로,
2차(실동작)는 테스트 클러스터가 있을 때만 가능 — 문서에는 절차만 남긴다.

- **A.1 (fence-peer 핸들러)**: (1) 단위 테스트 —
  `disk_options.fencing="resource-only"`인 리소스로 `generate_ansible_playbook()`
  결과에 `global_common.conf`/`fence-peer`/`crm-fence-peer.9.sh` 문자열이
  포함되는지 확인. `fencing="dont-care"` 등 다른 값이면 handlers 블록이
  아예 생략되는지도 함께 확인. (2) 실동작(테스트 클러스터 보유 시) — 가이드
  7.1절 재현 절차 그대로, **3노드 테스트 클러스터**(이 앱의 기본 대상,
  실제 사고도 3노드 `cms-t2-b05-service01~03`에서 발생)에서 한 노드에
  `pcs node standby` 실행 후 DRBD promote가 수 초 내 완료되는지 확인
  (핸들러 미등록 상태에서는 90초+ 타임아웃 재현되어야 회귀 테스트로서
  의미가 있음).
- **A.2 (op timeout / clone-max)**: 단위 테스트로 생성된 pcs 명령에
  `op demote`/`op promote`/`op start`/`op stop`/`op notify` 라인이 전부
  존재하고 값이 프리셋 표(A.2 참고) 범위인지 확인. 노드 3개짜리
  `ClusterConfig`로 생성했을 때 `clone-max=3`(2가 아니라)이 나오는지도
  회귀 테스트로 반드시 포함 — 이게 A.2에서 찾은 실제 버그다.
- **A.3/A.4/A.5 (Ansible 래퍼 멱등성/부트스트랩/STONITH)**: (1) 단위
  테스트 — 생성된 Ansible YAML을 파싱해서 `pcs resource config`/
  `pcs constraint --full`/`pcs stonith status` 사전 조회 태스크가 존재하고,
  이후 생성 태스크마다 `is not search(...)` 조건이 붙어 있는지 확인. (2)
  실동작 — 빈 클러스터(노드만 있고 pcs cluster 없음)에 생성된 부트스트랩
  플레이북을 **연속 두 번 실행**해서 (a) 1차 실행 후 `pcs status`가 전
  노드 Online, (b) 2차 실행이 에러 없이 끝나고 아무것도 재생성하지 않는지
  (idempotency 회귀 테스트의 핵심 기준) 둘 다 확인. STONITH는 실제 IPMI가
  있는 랩 환경에서만 검증 가능 — 없으면 생성된 명령 문자열 검토로 대체.
- **A.6 (anchor/follower)**: `anchor_clone`/`follower_clone` 입력으로
  생성했을 때 `pcs constraint colocation add <follower> ... with
  Promoted <anchor> ...` 순서로 나오는지(반대로 나오면 안 됨) 단위
  테스트로 확인.
- **A.7 (유지보수 스크립트)**: 생성된 스크립트에 5.1절 13단계가 순서대로
  전부 있는지, 각 노드 처리 사이에 `any_errors_fatal`류 중단 장치(또는
  최소한 실패 시 다음 노드로 넘어가지 않는다는 경고 주석)가 있는지 확인.
- **A.8 (인벤토리 그룹 검증)**: 생성된 플레이북/문서에 `ansible-inventory
  --graph` 사전 점검 안내가 실제로 포함되는지 확인 (동작 검증이 아니라
  문서화 여부 확인 수준).
- **A.9 (방화벽 포트)**: 생성된 Ansible에 `firewalld` 태스크로
  `5404-5405/udp`, `2224/tcp`가 포함되는지 단위 테스트로 확인. 실동작은
  방화벽을 기본 상태(닫힘)로 초기화한 새 노드에 배포 후 `pcs cluster
  setup`이 실제로 통신에 성공하는지로 검증.
- **B.1 (nft established/DNS bypass)**: 생성된 `.nft`를 테스트 노드에
  적용한 뒤, ipvlan 컨테이너 안에서 `curl https://example.com`과 `dig
  example.com`이 정상 응답하는지 확인 (이 규칙이 없으면 응답 패킷이
  드롭되어 타임아웃 나야 정상 — 회귀 확인용으로 규칙 제거 상태와 비교).
- **B.2 (전 노드 동일 배포)**: 생성된 Ansible 플레이북을 2~3노드 테스트
  클러스터에 실행한 뒤, 모든 노드에서 `/etc/nftables/ipvlan_l2.nft`
  내용이 동일한지(`md5sum` 비교) 확인. 이후 pacemaker로 서비스를 다른
  노드로 failover 시키고, 새로 옮겨간 노드에서도 해당 pod IP 트래픽이
  기존과 동일하게 허용되는지 확인 — 이게 이 기능의 존재 이유이므로
  failover 시나리오까지 반드시 검증한다.
- **C (Install/AddCapability)**: 단위 테스트로 `wanted_by`/
  `add_capabilities` 설정 시 `.container` 출력에 `[Install]
  WantedBy=...`/`AddCapability=...` 라인이 정확히 나오는지 확인. 실제
  `systemctl daemon-reload` 후 `systemctl status <name>.service`로
  Install 섹션이 인식되는지도 확인.
- **D (LINSTOR)**: 아직 설계 스케치 단계라 구체적 테스트 계획은 D.1/D.2의
  "결정 필요" 항목이 정해진 뒤 채운다. D.1(감지)만 먼저 구현한다면, 최소
  단위 테스트는 "linstor CLI가 있는 노드 → `storage_backend=linstor`",
  "없는 노드 → `kernel-module`"로 감지 결과가 갈리는지 확인하는 수준부터
  시작.

각 생성기 함수는 순수 함수(문자열 반환)이므로, 새 로직마다
`src/generators/*.rs` 하단에 `#[cfg(test)] mod tests` 유닛 테스트를
추가하는 것을 기본 원칙으로 한다 (기존 코드에 테스트가 있는지는 구현
시작 시 먼저 확인).

---

## 범위에서 제외한 것

- **2-node 클러스터는 다루지 않는다** — 사용자 결정(2026-09). 이 앱의
  기본/공식 대상은 **3-node 클러스터**다. 2노드 구성은 쿼럼 tie-breaker나
  쿼럼 디바이스 등 별도 고려사항이 필요해 더 어렵기 때문에 이번 계획의
  판단 기준에서 제외한다 (A.2의 `clone-max` 버그 판단, A.4의
  `no_quorum_policy=stop` 결정 모두 이 3-node 기준을 전제로 한다).
  `README.md`("3-nodes")와 `CLAUDE.md`("2-node") 간 표기 불일치는 이
  결정으로 해소되며, `CLAUDE.md` 쪽 표기를 추후 "3-node"로 맞추는 것을
  권장한다 (이 PLAN.md에서 직접 수정하지는 않음).
- `~/playbooks_cmst2/` 하위 어떤 파일도 수정하지 않음 (읽기 전용 참고
  자료 — `container_playbooks/`, `docs/pacemaker-drbd-guide.md` 포함).
- **Pod 내부 방화벽 사이드카(2차 방어선)는 구현하지 않는다** — 사용자
  결정(2026-09). 모든 방화벽 제어는 호스트 nft netdev table 필터
  (Section B) 하나로 일원화한다. 대신 B.2에서 그 단일 필터를 클러스터
  전 노드에 동일하게 배포하는 것을 보장한다 (서비스가 failover로 이동해도
  방화벽 구멍이 안 생기도록).
- bdii/condor/dcache 등 서비스 고유 설정 파일(HTCondor `.conf`, dCache
  설정 등) 생성 기능은 이 앱의 범용 지향과 맞지 않아 이식하지 않음.
- Quadlet에서 실제 사용 사례가 없는 `HealthCmd=`/`Secret=`/
  `EnvironmentFile=` 등은 이번 계획에서 제외 (필요해지면 추후 추가).
- **LINSTOR 소스 컴파일/빌드/설치 자동화는 이 저장소에 넣지 않는다** —
  사용자 결정(2026-09). 별도 코드 + 별도 GitHub Actions 워크플로우로
  분리하고, 이 앱은 그 산출물을 소비하는 역할만 한다 (D.3 참고). 이
  저장소가 다루는 건 D.1(설치 환경 감지)과 D.2(모델/생성기 확장)뿐이다.

## 구현 순서 제안

1. **A.1 (fence-peer 핸들러) — 최우선. ✅ 완료.** 사용자가 실제로 겪은
   문제와 직결되며, 기본값(`fencing=resource-only`)에서 이미 발생 중인
   버그였다. `R09.drbd.yml`/`drbd_global_common.conf.j2`를 그대로 이식.
2. A.2 (op timeout + `clone-max` 버그 수정) — ✅ 완료. A.1과 같은 파일을
   다루므로 이어서 진행. `clone-max` 하드코딩은 3노드 클러스터에서 바로
   터지는 버그였다.
3. B.1, B.2 (nft bypass, 전 노드 배포 Ansible) — ✅ 완료. 같은 라우트/
   생성기 파일이라 함께 진행.
4. A.3+A.4+A.5+A.9 (Ansible 래퍼 멱등성/부트스트랩/STONITH/방화벽) — ✅
   완료. 넷 다 `src/routes/pacemaker.rs`의 같은 Ansible 래퍼 생성 부분을
   다시 쓰는 작업이라 한 번에 묶어서 진행 (R02/R11 구조를 그대로 이식).
   구현 중 발견한 실제 버그(YAML `when:`/`ansible.builtin.command:` 값에
   사용자 입력의 `'`/`"`가 그대로 들어가 파싱이 깨지는 문제)를
   `yaml_squote()`/`yaml_dquote()` 헬퍼로 수정 — 라이브 서버 POST +
   `python3 yaml.safe_load`로 재현 및 수정 확인.
5. A.6 (anchor/follower 안내) — ✅ 완료. Pacemaker UI 확장, 위 4번과
   독립적.
6. C (Quadlet Install/AddCapability) — ✅ 완료. 우선순위는 낮았지만
   완료함. 구현 중 컨테이너 폼 제출 자체가 항상 실패하던 무관한
   기존 버그(`podman_inspect_to_quadlet` 우선 시도 순서 오류)를
   함께 발견해 수정.
7. A.7 (유지보수 스크립트) — ✅ 완료. A.8(인벤토리 검증 주석)도 ✅ 완료.
8. **D.1 (LINSTOR 설치 환경 감지)** — VM 테스트 클러스터가 LINSTOR로
   구성될 예정이므로 실제로는 우선순위가 낮지 않지만, "결정 필요" 항목
   (LINBIT EL9 공개 저장소 범위, 노드별 백엔드 혼재 허용 여부)이 먼저
   확인돼야 착수 가능. D.2(모델/생성기 확장)는 D.1 이후, 그리고 VM
   클러스터에서 LINSTOR의 실제 `.res` 관리 방식을 확인한 뒤 설계를
   구체화한다.

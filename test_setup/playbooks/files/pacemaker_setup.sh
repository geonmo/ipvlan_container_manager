#!/bin/bash
# ipvlan_container_manager 테스트 클러스터(test_cluster) 부트스트랩 스크립트
# deploy_pacemaker.yml 의 두 번째 play가 대표 노드(인벤토리 첫 번째 노드)에서 실행한다.
# 이 시점에는 "pcs host auth" 가 이미 끝나 있다고 가정한다.
set -euo pipefail

CLUSTER_NAME="test_cluster"
# corosync 는 반드시 private 네트워크(10.0.0.0/24, eth2)로 통신해야 하므로
# 노드 이름(node0X.build.test, pacemaker 상의 식별자)과 별개로 addr= 에 private IP를
# 명시적으로 지정한다. (참고: 이 addr= 지정은 노드 자기참조 /etc/hosts 이슈
# — 00-fix-etc-hosts.yml 참고 — 와 무관하게, corosync 트래픽을 public 이 아닌
# private 망으로 강제 고정하기 위해서도 필요하다.)
NODE1="node01.build.test addr=10.0.0.3"
NODE2="node02.build.test addr=10.0.0.4"
NODE3="node03.build.test addr=10.0.0.5"

if pcs status >/dev/null 2>&1; then
  echo "클러스터가 이미 구성되어 있습니다. 초기화를 건너뜁니다."
else
  pcs cluster setup "${CLUSTER_NAME}" ${NODE1} ${NODE2} ${NODE3}
  pcs cluster start --all
  pcs cluster enable --all
fi

# 테스트 환경: STONITH 장비(IPMI 등)가 없으므로 비활성화, 쿼럼 미달도 무시
pcs property set stonith-enabled=false
pcs property set no-quorum-policy=ignore

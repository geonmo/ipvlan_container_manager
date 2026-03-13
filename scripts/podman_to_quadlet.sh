#!/bin/bash
# ============================================================
# podman_to_quadlet.sh
# 실행 중인 Podman 컨테이너를 Quadlet 유닛 파일로 변환
#
# 사용법:
#   ./podman_to_quadlet.sh [OPTIONS] [CONTAINER_NAME...]
#
# 옵션:
#   -o, --output-dir DIR   출력 디렉토리 (기본: ./quadlet-units)
#   -i, --install          /etc/containers/systemd/ 에 직접 설치
#   -n, --network NAME     IPVLAN 네트워크 이름 지정
#   -d, --drbd RESOURCE    연결할 DRBD 리소스 이름 (주석으로 추가)
#   -h, --help             도움말 출력
#
# 예시:
#   ./podman_to_quadlet.sh myapp mydb
#   ./podman_to_quadlet.sh --install --network ipvlan0 myapp
# ============================================================

set -euo pipefail

# ── 기본값 설정 ────────────────────────────────────────────────
OUTPUT_DIR="./quadlet-units"
INSTALL=false
NETWORK_NAME=""
DRBD_RESOURCE=""
CONTAINERS=()

# ── 색상 출력 ──────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; exit 1; }

# ── 인수 파싱 ──────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        -o|--output-dir)
            OUTPUT_DIR="$2"; shift 2 ;;
        -i|--install)
            INSTALL=true; shift ;;
        -n|--network)
            NETWORK_NAME="$2"; shift 2 ;;
        -d|--drbd)
            DRBD_RESOURCE="$2"; shift 2 ;;
        -h|--help)
            head -30 "$0" | grep '^#' | sed 's/^# \{0,1\}//'
            exit 0 ;;
        -*)
            error "알 수 없는 옵션: $1" ;;
        *)
            CONTAINERS+=("$1"); shift ;;
    esac
done

# ── 사전 검사 ──────────────────────────────────────────────────
command -v podman &>/dev/null || error "podman이 설치되어 있지 않습니다."
command -v jq &>/dev/null    || error "jq가 필요합니다: dnf install -y jq"

# 컨테이너 목록이 없으면 실행 중인 전체 컨테이너 사용
if [[ ${#CONTAINERS[@]} -eq 0 ]]; then
    info "실행 중인 모든 컨테이너를 대상으로 변환합니다..."
    mapfile -t CONTAINERS < <(podman ps --format '{{.Names}}')
    [[ ${#CONTAINERS[@]} -eq 0 ]] && error "실행 중인 컨테이너가 없습니다."
fi

# ── 출력 디렉토리 생성 ─────────────────────────────────────────
mkdir -p "$OUTPUT_DIR"
info "출력 디렉토리: $OUTPUT_DIR"

# ── 각 컨테이너 변환 ───────────────────────────────────────────
for CONTAINER in "${CONTAINERS[@]}"; do
    info "변환 중: $CONTAINER"

    # inspect 데이터 가져오기
    INSPECT=$(podman inspect "$CONTAINER" 2>/dev/null) \
        || { warn "$CONTAINER: inspect 실패 (건너뜀)"; continue; }

    # 기본 정보 추출
    IMAGE=$(echo "$INSPECT" | jq -r '.[0].ImageName // .[0].Config.Image // "unknown"')
    NAME=$(echo "$INSPECT"  | jq -r '.[0].Name' | sed 's|^/||')
    UNIT_FILE="$OUTPUT_DIR/${NAME}.container"

    # 환경변수
    ENV_VARS=$(echo "$INSPECT" | jq -r '.[0].Config.Env[]? // empty' 2>/dev/null || true)

    # 볼륨 마운트
    MOUNTS=$(echo "$INSPECT" | jq -r '.[0].Mounts[]? | "\(.Source):\(.Destination):\(.Mode)"' 2>/dev/null || true)

    # 포트 매핑
    PORTS=$(echo "$INSPECT" | \
        jq -r '.[0].NetworkSettings.Ports | to_entries[]? |
               "\(.value[0].HostPort):\(.key)"' 2>/dev/null || true)

    # 네트워크 및 IP
    NET_NAME=$(echo "$INSPECT" | jq -r '.[0].NetworkSettings.Networks | keys[0] // ""' 2>/dev/null || true)
    NET_IP=$(echo "$INSPECT"   | jq -r '.[0].NetworkSettings.Networks[keys[0]].IPAddress // ""' 2>/dev/null || true)

    # ── .container 파일 작성 ───────────────────────────────────
    {
        echo "# Quadlet Container Unit"
        echo "# 원본 컨테이너: $NAME"
        echo "# 생성 일시: $(date '+%Y-%m-%d %H:%M:%S')"
        [[ -n "$DRBD_RESOURCE" ]] && echo "# DRBD 리소스: $DRBD_RESOURCE"
        echo ""

        # Unit 섹션 (의존성)
        echo "[Unit]"
        echo "Description=Podman container - $NAME"
        if [[ -n "$DRBD_RESOURCE" ]]; then
            echo "# DRBD 마운트 완료 후 시작 (Pacemaker가 관리하는 경우 주석 해제)"
            echo "# After=drbd@${DRBD_RESOURCE}.target"
        fi
        echo ""

        echo "[Container]"
        echo "Image=$IMAGE"

        # 네트워크 설정
        if [[ -n "$NETWORK_NAME" ]]; then
            echo "Network=${NETWORK_NAME}.network"
        elif [[ -n "$NET_NAME" && "$NET_NAME" != "null" ]]; then
            echo "Network=${NET_NAME}.network"
        fi

        if [[ -n "$NET_IP" && "$NET_IP" != "null" && -n "$NET_IP" ]]; then
            echo "IP=$NET_IP"
        fi

        # 포트 매핑
        if [[ -n "$PORTS" ]]; then
            while IFS= read -r port; do
                [[ -n "$port" ]] && echo "PublishPort=$port"
            done <<< "$PORTS"
        fi

        # 환경변수 (PATH, HOME 등 시스템 변수 제외)
        if [[ -n "$ENV_VARS" ]]; then
            while IFS= read -r env; do
                # 시스템 기본 환경변수 필터링
                case "$env" in
                    PATH=*|HOME=*|HOSTNAME=*|TERM=*|container=*) continue ;;
                    *=*) echo "Environment=$env" ;;
                esac
            done <<< "$ENV_VARS"
        fi

        # 볼륨 마운트
        if [[ -n "$MOUNTS" ]]; then
            while IFS= read -r mount; do
                [[ -n "$mount" ]] && echo "Volume=$mount"
            done <<< "$MOUNTS"
        fi

        echo ""
        echo "[Service]"
        echo "Restart=on-failure"
        echo "TimeoutStartSec=60"
        echo "TimeoutStopSec=60"
        echo ""
        echo "[Install]"
        echo "WantedBy=default.target"

    } > "$UNIT_FILE"

    success "생성: $UNIT_FILE"
done

# ── 설치 모드: /etc/containers/systemd/ 에 복사 ────────────────
if [[ "$INSTALL" == true ]]; then
    INSTALL_DIR="/etc/containers/systemd"
    mkdir -p "$INSTALL_DIR"
    info "설치 중: $INSTALL_DIR"

    for f in "$OUTPUT_DIR"/*.container; do
        [[ -f "$f" ]] || continue
        cp "$f" "$INSTALL_DIR/"
        success "설치: $INSTALL_DIR/$(basename "$f")"
    done

    info "systemd 데몬 리로드..."
    systemctl daemon-reload
    success "설치 완료. 'systemctl list-units --type=service' 로 확인하세요."
fi

echo ""
success "변환 완료! 생성된 파일:"
ls -1 "$OUTPUT_DIR"/*.container 2>/dev/null | while read -r f; do
    echo "  - $f"
done

echo ""
info "다음 단계:"
echo "  1. 생성된 .container 파일을 검토하고 필요시 수정"
echo "  2. /etc/containers/systemd/ 에 복사: sudo cp $OUTPUT_DIR/*.container /etc/containers/systemd/"
echo "  3. systemctl daemon-reload"
echo "  4. Pacemaker에 systemd 리소스로 등록 (웹 UI의 Pacemaker 설정 모듈 사용)"

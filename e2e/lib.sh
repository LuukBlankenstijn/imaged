set -euo pipefail

E2E_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(dirname "$E2E_DIR")
OUT_DIR=${E2E_OUT:-$E2E_DIR/out}
STEP_LOG=$OUT_DIR/steps.txt

CABLE_NET=192.168.213
SERVER_IP=$CABLE_NET.1
CABLE_CIDR=$SERVER_IP/24
IMAGED_URL=http://$SERVER_IP:8080

log() {
    printf '%s [%s] %s\n' "$(date +%T)" "${CURRENT_STEP:-e2e}" "$*" >&2
}

fail() {
    log "FAIL: $*"
    exit 1
}

relative() {
    printf '%s' "${1#"$REPO_DIR"/}"
}

require_commands() {
    local missing=()
    local command
    for command in "$@"; do
        command -v "$command" >/dev/null || missing+=("$command")
    done
    if ((${#missing[@]})); then
        fail "missing commands: ${missing[*]} (run inside: nix develop $REPO_DIR#e2e)"
    fi
}

run_step() {
    local name=$1
    shift
    CURRENT_STEP=$name
    local started=$SECONDS
    log "start"
    "$@"
    local elapsed=$((SECONDS - started))
    printf '%5ss  %s\n' "$elapsed" "$name" >>"$STEP_LOG"
    log "done in ${elapsed}s"
    CURRENT_STEP=
}

print_step_summary() {
    log "step timings:"
    sed 's/^/  /' "$STEP_LOG" >&2
}

declare -A PIDS=()
declare -A LOGS=()
declare -A WATCHED=()
VM_NAMES=()

start_background() {
    local name=$1 logfile=$2
    shift 2
    "$@" >"$logfile" 2>&1 &
    PIDS[$name]=$!
    LOGS[$name]=$logfile
    WATCHED[$name]=1
    log "started $name (pid ${PIDS[$name]}, log $(relative "$logfile"))"
}

running() {
    kill -0 "${PIDS[$1]}" 2>/dev/null
}

unwatch() {
    unset "WATCHED[$1]"
}

assert_watched_alive() {
    local name
    for name in "${!WATCHED[@]}"; do
        if ! running "$name"; then
            log "$name exited unexpectedly; last lines of $(relative "${LOGS[$name]}"):"
            tail -n 30 "${LOGS[$name]}" >&2 || true
            fail "$name is not running"
        fi
    done
}

stop_process() {
    unwatch "$1"
    kill "${PIDS[$1]}" 2>/dev/null || true
    wait "${PIDS[$1]}" 2>/dev/null || true
    log "stopped $1"
}

wait_for_exit() {
    local name=$1 timeout=$2
    unwatch "$name"
    local deadline=$((SECONDS + timeout))
    while running "$name"; do
        ((SECONDS < deadline)) || fail "timed out after ${timeout}s waiting for $name to exit"
        sleep 1
    done
    wait "${PIDS[$name]}" 2>/dev/null || true
    log "$name exited"
}

wait_until() {
    local timeout=$1 what=$2
    shift 2
    local deadline=$((SECONDS + timeout))
    log "waiting up to ${timeout}s for $what"
    until "$@"; do
        assert_watched_alive
        ((SECONDS < deadline)) || fail "timed out after ${timeout}s waiting for $what"
        sleep 2
    done
}

report_failure_context() {
    local name logfile
    for name in "${VM_NAMES[@]}"; do
        logfile=$OUT_DIR/$name.serial.log
        [[ -s $logfile ]] || continue
        log "last lines of $(relative "$logfile"):"
        tr -d '\r' <"$logfile" | sed 's/\x1b\[[0-9;?]*[A-Za-z]//g' | grep -v '^[[:space:]]*$' | tail -n 15 | sed 's/^/    /' >&2 || true
    done
    log "all logs are in $(relative "$OUT_DIR")/"
}

stop_background_processes() {
    local status=$?
    trap - EXIT INT TERM
    local pid
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    for pid in "${PIDS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    ((status == 0)) || report_failure_context
    exit "$status"
}

enter_cable_namespace() {
    if [[ -z ${E2E_IN_NAMESPACE:-} ]]; then
        exec unshare --user --map-root-user --net --pid --fork --mount-proc --kill-child -- \
            env E2E_IN_NAMESPACE=1 "$@"
    fi
    trap stop_background_processes EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
}

setup_cable_bridge() {
    ip link set lo up
    ip link add br0 type bridge forward_delay 0 stp_state 0 mcast_snooping 0
    ip addr add "$CABLE_CIDR" dev br0
    ip link set br0 up
    log "br0 up with $CABLE_CIDR in an isolated user+network namespace"
}

add_cable_tap() {
    local tap=$1
    ip tuntap add dev "$tap" mode tap
    ip link set "$tap" master br0
    ip link set "$tap" up
}

write_dnsmasq_config() {
    cat >"$OUT_DIR/dnsmasq.conf" <<EOF
interface=br0
bind-interfaces
port=0
dhcp-range=$CABLE_NET.100,$CABLE_NET.199,255.255.255.0,1h
dhcp-authoritative
dhcp-leasefile=$OUT_DIR/dnsmasq.leases
dhcp-userclass=set:ipxe,iPXE
dhcp-match=set:bios,option:client-arch,0
dhcp-match=set:efi32,option:client-arch,6
dhcp-match=set:efibc,option:client-arch,7
dhcp-match=set:efi64,option:client-arch,9
dhcp-match=set:efiarm64,option:client-arch,11
dhcp-boot=tag:!ipxe,tag:bios,undionly.kpxe,,$SERVER_IP
dhcp-boot=tag:!ipxe,tag:efi32,ipxe.efi,,$SERVER_IP
dhcp-boot=tag:!ipxe,tag:efibc,ipxe.efi,,$SERVER_IP
dhcp-boot=tag:!ipxe,tag:efi64,ipxe.efi,,$SERVER_IP
dhcp-boot=tag:!ipxe,tag:efiarm64,ipxe-arm64.efi,,$SERVER_IP
dhcp-boot=tag:ipxe,$IMAGED_URL/boot/boot.ipxe
log-dhcp
EOF
}

start_dhcp_server() {
    write_dnsmasq_config
    start_background dnsmasq "$OUT_DIR/dnsmasq.log" \
        dnsmasq --no-daemon --user=root --pid-file --conf-file="$OUT_DIR/dnsmasq.conf"
}

api_request() {
    local path=$1 response
    shift
    if ! response=$(curl -sS --noproxy '*' --fail-with-body --max-time 10 "$@" "$IMAGED_URL$path" 2>&1); then
        log "request $path failed: $response"
        return 1
    fi
    printf '%s' "$response"
}

api_get() {
    api_request "$1"
}

api_post() {
    api_request "$1" -H 'content-type: application/json' -d "$2"
}

api_upload() {
    api_request "$1" --max-time 300 -H 'content-type: application/octet-stream' --data-binary "@$2"
}

api_download() {
    local path=$1 file=$2
    if ! curl -sS --noproxy '*' --fail-with-body --max-time 300 -D "$file.headers" -o "$file" "$IMAGED_URL$path"; then
        fail "download $path failed: $(cat "$file" 2>/dev/null)"
    fi
}

imaged_answers() {
    curl -sS --noproxy '*' -o /dev/null --max-time 2 "$IMAGED_URL/boot/boot.ipxe" 2>/dev/null
}

host_id_for_mac() {
    api_get /api/ui/hosts | jq -r --arg mac "${1,,}" \
        '.[] | select((.mac_address | ascii_downcase | gsub("-"; ":")) == $mac) | .id'
}

host_registered() {
    [[ -n $(host_id_for_mac "$1") ]]
}

image_json() {
    api_get /api/ui/images | jq -c --argjson id "$1" '.[] | select(.id == $id)'
}

image_field() {
    api_get /api/ui/images | jq -r --argjson id "$1" ".[] | select(.id == \$id) | $2"
}

task_field() {
    api_get "/api/ui/tasks?limit=200" | jq -r --argjson id "$1" ".tasks[] | select(.id == \$id) | $2"
}

image_ready() {
    local status
    status=$(image_field "$1" .status)
    case $status in
        ready) return 0 ;;
        faulted) fail "image $1 faulted: $(image_field "$1" .error_message)" ;;
        *) return 1 ;;
    esac
}

task_done() {
    local state
    state=$(task_field "$1" .state)
    case $state in
        done) return 0 ;;
        failed | cancelled | partial) fail "task $1 ended $state: $(task_field "$1" '[.hosts[].error // empty] | join("; ")')" ;;
        *) return 1 ;;
    esac
}

FIRMWARE=${E2E_FIRMWARE:-bios}
case $FIRMWARE in
    bios | uefi) ;;
    *) fail "unknown E2E_FIRMWARE '$FIRMWARE', expected bios or uefi" ;;
esac
FIRMWARE_ARGS=()

prepare_firmware() {
    local name=$1
    case $FIRMWARE in
        bios)
            printf '\xf8\x03' >"$OUT_DIR/sercon-port"
            FIRMWARE_ARGS=(
                -fw_cfg "name=etc/sercon-port,file=$OUT_DIR/sercon-port"
                -chardev "file,id=seabios,path=$OUT_DIR/$name.seabios.log"
                -device isa-debugcon,iobase=0x402,chardev=seabios
            )
            ;;
        uefi)
            local share
            share=$(dirname "$(command -v qemu-system-x86_64)")/../share/qemu
            cp --no-preserve=mode "$share/edk2-i386-vars.fd" "$OUT_DIR/$name.vars.fd"
            FIRMWARE_ARGS=(
                -drive "if=pflash,format=raw,readonly=on,file=$share/edk2-x86_64-code.fd"
                -drive "if=pflash,format=raw,file=$OUT_DIR/$name.vars.fd"
            )
            ;;
    esac
}

start_network_boot_vm() {
    local name=$1 mac=$2 disk=$3 tap=$4
    add_cable_tap "$tap"
    prepare_firmware "$name"
    VM_NAMES+=("$name")
    start_background "$name" "$OUT_DIR/$name.qemu.log" \
        qemu-system-x86_64 \
        -name "$name" \
        -machine q35,accel=kvm \
        -cpu host \
        -smp 2 \
        -m 1G \
        -nodefaults \
        -display none \
        -no-reboot \
        "${FIRMWARE_ARGS[@]}" \
        -serial "file:$OUT_DIR/$name.serial.log" \
        -netdev "tap,id=cable,ifname=$tap,script=no,downscript=no" \
        -device "virtio-net-pci,netdev=cable,mac=$mac,bootindex=1" \
        -drive "file=$disk,format=raw,if=none,id=disk" \
        -device virtio-blk-pci,drive=disk
    log "$name network-booting ($FIRMWARE, mac $mac, disk $(relative "$disk"), serial log $(relative "$OUT_DIR/$name.serial.log"))"
}

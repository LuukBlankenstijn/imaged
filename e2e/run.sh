#!/usr/bin/env bash
# usage: nix develop .#e2e -c e2e/run.sh  (env: E2E_FIRMWARE=bios|uefi, E2E_OUT, CARGO_TARGET_DIR, IMAGED_BOOT_ASSETS)
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$E2E_DIR/disk.sh"

IMAGED_BOOT_ASSETS=$(realpath -m "${IMAGED_BOOT_ASSETS:-$REPO_DIR/assets}")
export IMAGED_BOOT_ASSETS
TARGET_DIR=$(cd "$REPO_DIR" && realpath -m "${CARGO_TARGET_DIR:-target}")
LINUX_TARGET=x86_64-unknown-linux-gnu
IMAGED_SERVER=$TARGET_DIR/$LINUX_TARGET/debug/imaged-server
IMAGED_PACK=$TARGET_DIR/$LINUX_TARGET/debug/imaged-pack
TAILWIND_CSS=$REPO_DIR/crates/web/assets/tailwind.css

CAPTURE_MAC=52:54:00:e2:e0:01
PACKED_MAC=52:54:00:e2:e0:02
CAPTURED_MAC=52:54:00:e2:e0:03
SOURCE_DISK=$OUT_DIR/source.raw
PACKED_DISK=$OUT_DIR/deploy-packed.raw
CAPTURED_DISK=$OUT_DIR/deploy-captured.raw
SCRIPT_DIR=$OUT_DIR/scripts
SCRIPT_NAME=e2e-hostname
PACKED_ARCHIVE=$OUT_DIR/packed.imaged
PACKED_EXPORT=$OUT_DIR/packed-export.imaged
CAPTURED_ARCHIVE=$OUT_DIR/captured.imaged
COMPARE_DIR=$OUT_DIR/compare
IMAGED_DATA=$OUT_DIR/imaged-data

clean_previous_run() {
    rm -rf "$IMAGED_DATA" "$OUT_DIR/verify" "$COMPARE_DIR" "$SCRIPT_DIR" "$STEP_LOG" \
        "$OUT_DIR"/*.log "$OUT_DIR"/*.raw "$OUT_DIR"/*.fd "$OUT_DIR"/*.imaged "$OUT_DIR"/*.imaged.* \
        "$OUT_DIR"/dnsmasq.* "$OUT_DIR/sercon-port"
}

describe_checkout() {
    git -C "$REPO_DIR" describe --always --dirty 2>/dev/null || echo unknown
}

check_boot_assets() {
    local asset
    for asset in x86_64/vmlinuz x86_64/initramfs.cpio.xz arm64/vmlinuz arm64/initramfs.cpio.xz ipxe.efi ipxe-arm64.efi undionly.kpxe; do
        [[ -f $IMAGED_BOOT_ASSETS/$asset ]] || fail "missing $IMAGED_BOOT_ASSETS/$asset; build the boot assets (build-initramfs x86_64) or copy real ones into $IMAGED_BOOT_ASSETS"
    done
    local initramfs=$IMAGED_BOOT_ASSETS/x86_64/initramfs.cpio.xz
    if (($(stat -c %s "$initramfs") < 1048576)); then
        fail "$initramfs is a placeholder; run build-initramfs x86_64"
    fi
    log "initramfs $initramfs: sha256 $(sha256sum "$initramfs" | cut -d' ' -f1), modified $(date -r "$initramfs" '+%F %T %Z')"
    local cpio=(cpio -it)
    command -v cpio >/dev/null || cpio=("$E2E_BUSYBOX" cpio -t)
    local listing binary
    listing=$(xz -dc "$initramfs" | "${cpio[@]}" 2>/dev/null) || fail "cannot list $initramfs; run build-initramfs x86_64"
    for binary in bin/debugfs bin/imaged-agent; do
        grep -qxE "(\./)?$binary" <<<"$listing" ||
            fail "$initramfs has no $binary; it was built from an older tree, run build-initramfs x86_64"
    done
    log "initramfs carries bin/debugfs and bin/imaged-agent"
}

build_binaries() {
    if [[ ! -f $TAILWIND_CSS ]]; then
        mkdir -p "$(dirname "$TAILWIND_CSS")"
        : >"$TAILWIND_CSS"
        log "created an empty $(relative "$TAILWIND_CSS") so the dashboard compiles"
    fi
    (cd "$REPO_DIR" && SQLX_OFFLINE=true cargo build --locked --target "$LINUX_TARGET" -p imaged-server --features server)
    (cd "$REPO_DIR" && SQLX_OFFLINE=true cargo build --locked --target "$LINUX_TARGET" -p imaged-pack)
    mkdir -p "$(dirname "$IMAGED_SERVER")/public"
    log "built $IMAGED_SERVER and $IMAGED_PACK from $(describe_checkout)"
}

build_disks() {
    build_test_disk "$SOURCE_DISK"
    build_blank_disk "$PACKED_DISK"
    build_blank_disk "$CAPTURED_DISK"
}

log_archive_entries() {
    local entries
    entries=$(tar -tvf "$1") || fail "$(relative "$1") is not a tar archive"
    log "$(relative "$1") ($(stat -c %s "$1") bytes):"
    printf '    %s\n' "${entries//$'\n'/$'\n    '}" >&2
}

write_test_script() {
    mkdir -p "$SCRIPT_DIR"
    cat >"$SCRIPT_DIR/$SCRIPT_NAME.sh" <<'EOF'
#!/bin/sh
set -e
echo "$IMAGED_INPUT_HOSTNAME" >/etc/hostname
EOF
    jq -n '[{name: "hostname", label: "Hostname", kind: "text", required: true, per_machine: true}]' \
        >"$SCRIPT_DIR/$SCRIPT_NAME.inputs.json"
}

pack_source_disk() {
    write_test_script
    if ! "$IMAGED_PACK" "$SOURCE_DISK" --name e2e-packed --script "$SCRIPT_DIR/$SCRIPT_NAME.sh" -o "$PACKED_ARCHIVE" \
        >"$OUT_DIR/imaged-pack.log" 2>&1; then
        tail -n 30 "$OUT_DIR/imaged-pack.log" >&2 || true
        fail "imaged-pack failed, see $(relative "$OUT_DIR/imaged-pack.log")"
    fi
    log_archive_entries "$PACKED_ARCHIVE"
}

start_imaged_server() {
    cat >"$OUT_DIR/imaged.toml" <<EOF
data_dir = "$IMAGED_DATA"
multicast_interface = "br0"
public_url = "http://$SERVER_IP:8080"

[listen]
machine = "$SERVER_IP:8080"
tftp = "$SERVER_IP:69"
EOF
    start_background imaged-server "$OUT_DIR/imaged-server.log" \
        "$IMAGED_SERVER" --config "$OUT_DIR/imaged.toml"
    wait_until 60 "imaged-server to answer on $IMAGED_URL" imaged_answers
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

assert_root_partition() {
    local id=$1 what=$2 root
    root=$(image_field "$id" .root_partition)
    [[ $root == 2 ]] || fail "$what image $id has root partition $root, expected 2: $(image_json "$id")"
    log "$what image $id has root partition 2"
}

import_packed_archive() {
    PACKED_IMAGE_ID=$(api_upload /api/ui/images/import "$PACKED_ARCHIVE" | jq -r .id)
    log "imported $(relative "$PACKED_ARCHIVE") as image $PACKED_IMAGE_ID"
    wait_until 120 "image $PACKED_IMAGE_ID to become ready" image_ready "$PACKED_IMAGE_ID"
    log "image $PACKED_IMAGE_ID ready: $(image_json "$PACKED_IMAGE_ID")"
    assert_root_partition "$PACKED_IMAGE_ID" packed

    local scripts
    scripts=$(api_get "/api/ui/images/$PACKED_IMAGE_ID/scripts")
    jq -e --arg name "$SCRIPT_NAME" 'map(.name) == [$name]' <<<"$scripts" >/dev/null ||
        fail "packed image $PACKED_IMAGE_ID scripts are not [$SCRIPT_NAME]: $scripts"

    local library
    library=$(api_get /api/ui/scripts | jq -c --arg name "$SCRIPT_NAME" '.[] | select(.name == $name)')
    jq -e '[.inputs[] | select(.name == "hostname" and .kind == "text" and .required and .per_machine)] | length == 1' \
        <<<"$library" >/dev/null ||
        fail "library script $SCRIPT_NAME does not declare a required per-machine hostname input: $library"
    SCRIPT_ID=$(jq -r .id <<<"$library")
    log "library script $SCRIPT_NAME ($SCRIPT_ID) attached to image $PACKED_IMAGE_ID: $library"
}

capture_source_disk() {
    start_network_boot_vm capture "$CAPTURE_MAC" "$SOURCE_DISK" tap-capture
    wait_until 300 "the capture VM's agent to register" host_registered "$CAPTURE_MAC"
    local host_id
    host_id=$(host_id_for_mac "$CAPTURE_MAC")
    log "capture VM registered as host $host_id: $(api_get /api/ui/hosts | jq -c --argjson id "$host_id" '.[] | select(.id == $id)')"

    CAPTURED_IMAGE_ID=$(api_post /api/ui/images/create \
        "$(jq -nc --argjson host "$host_id" '{req: {name: "e2e-captured", host_id: $host}}')" | jq -r .id)
    log "created image $CAPTURED_IMAGE_ID, capture task dispatched"
    wait_until 600 "image $CAPTURED_IMAGE_ID to become ready" image_ready "$CAPTURED_IMAGE_ID"
    log "image $CAPTURED_IMAGE_ID ready: $(image_field "$CAPTURED_IMAGE_ID" '[.partitions[] | "p\(.partition_number) \(.fstype) \(.size_bytes)B"] | join(", ")')"

    local task_id
    task_id=$(api_get /api/ui/tasks | jq -r --argjson image "$CAPTURED_IMAGE_ID" '.[] | select(.image_id == $image and .type == "capture") | .id')
    wait_until 60 "capture task $task_id to finish" task_done "$task_id"
    stop_process capture
    assert_root_partition "$CAPTURED_IMAGE_ID" captured

    api_post /api/ui/images/scripts \
        "$(jq -nc --argjson image "$CAPTURED_IMAGE_ID" --argjson script "$SCRIPT_ID" '{req: {image_id: $image, script_ids: [$script]}}')" >/dev/null
    local scripts
    scripts=$(api_get "/api/ui/images/$CAPTURED_IMAGE_ID/scripts")
    jq -e --arg name "$SCRIPT_NAME" 'map(.name) == [$name]' <<<"$scripts" >/dev/null ||
        fail "attaching $SCRIPT_NAME to image $CAPTURED_IMAGE_ID left its scripts at: $scripts"
    log "attached library script $SCRIPT_NAME to image $CAPTURED_IMAGE_ID"
}

export_images() {
    api_download "/api/ui/images/$CAPTURED_IMAGE_ID/export" "$CAPTURED_ARCHIVE"
    log_archive_entries "$CAPTURED_ARCHIVE"
    api_download "/api/ui/images/$PACKED_IMAGE_ID/export" "$PACKED_EXPORT"
    log_archive_entries "$PACKED_EXPORT"
}

manifest_structure() {
    tar -xOf "$1" manifest.json | jq -S '[paths | map(if type == "number" then 0 else . end)] | unique'
}

compare_archives() {
    mkdir -p "$COMPARE_DIR"
    local archive name
    for archive in "$CAPTURED_ARCHIVE" "$PACKED_ARCHIVE"; do
        name=$(basename "$archive" .imaged)
        tar -tf "$archive" >"$COMPARE_DIR/$name.entries"
        tar -xOf "$archive" manifest.json | jq -S . >"$COMPARE_DIR/$name.manifest.json"
        manifest_structure "$archive" >"$COMPARE_DIR/$name.structure.json"
        log "$name entries: $(paste -sd' ' "$COMPARE_DIR/$name.entries")"
        log "$name manifest: $(jq -c . "$COMPARE_DIR/$name.manifest.json")"
    done
    diff "$COMPARE_DIR/captured.entries" "$COMPARE_DIR/packed.entries" >&2 ||
        fail "the captured export and the packed archive have different entries"
    diff "$COMPARE_DIR/captured.structure.json" "$COMPARE_DIR/packed.structure.json" >&2 ||
        fail "the captured and packed manifests have different key structures"
    log "the captured export and the packed archive have the same entries and manifest structure"
}

deploy_finished() {
    task_done "$DEPLOY_TASK_ID" && return 0
    running "$DEPLOY_VM" && return 1
    task_done "$DEPLOY_TASK_ID" || fail "$DEPLOY_VM VM exited while task $DEPLOY_TASK_ID is $(task_field "$DEPLOY_TASK_ID" .state)"
}

deploy_image() {
    local name=$1 mac=$2 disk=$3 tap=$4 image_id=$5 hostname=$6
    start_network_boot_vm "$name" "$mac" "$disk" "$tap"
    wait_until 300 "the $name VM's agent to register" host_registered "$mac"
    local host_id
    host_id=$(host_id_for_mac "$mac")
    log "$name VM registered as host $host_id"

    DEPLOY_VM=$name
    DEPLOY_TASK_ID=$(api_post /api/ui/hosts/deploy \
        "$(jq -nc --argjson host "$host_id" --argjson image "$image_id" --arg hostname "$hostname" \
            '{req: {id: $host, image_id: $image, inputs: {shared: {}, per_machine: {($host | tostring): {hostname: {value: $hostname}}}}}}')" |
        jq -r .id)
    log "deploy task $DEPLOY_TASK_ID dispatched: image $image_id to host $host_id with hostname $hostname"
    unwatch "$name"
    wait_until 600 "deploy task $DEPLOY_TASK_ID to finish" deploy_finished

    local runs
    runs=$(api_get "/api/ui/tasks/$DEPLOY_TASK_ID/hosts/$host_id/scripts")
    log "script runs: $(jq -c 'map({script_name, exit_code, output})' <<<"$runs")"
    jq -e --arg name "$SCRIPT_NAME" 'map(select(.script_name == $name and .exit_code == 0)) | length == 1' <<<"$runs" >/dev/null ||
        fail "deploy task $DEPLOY_TASK_ID did not run $SCRIPT_NAME successfully: $runs"

    log "deploy task $DEPLOY_TASK_ID done, waiting for the VM to reboot"
    wait_for_exit "$name" 120
}

deploy_packed() {
    deploy_image deploy-packed "$PACKED_MAC" "$PACKED_DISK" tap-packed "$PACKED_IMAGE_ID" e2e-packed
}

deploy_captured() {
    deploy_image deploy-captured "$CAPTURED_MAC" "$CAPTURED_DISK" tap-captured "$CAPTURED_IMAGE_ID" e2e-captured
}

prepare_outside_namespace() {
    require_commands cargo cc unshare ip qemu-system-x86_64 dnsmasq curl jq sgdisk sfdisk blkid mke2fs e2fsck debugfs \
        mkfs.vfat fsck.fat mcopy mkswap partclone.extfs partclone.vfat xz tar
    [[ -x ${E2E_BUSYBOX:-} ]] || fail "E2E_BUSYBOX must point at a static busybox binary (run inside: nix develop $REPO_DIR#e2e)"
    [[ -r /dev/kvm && -w /dev/kvm ]] || fail "/dev/kvm is not accessible"
    mkdir -p "$OUT_DIR"
    clean_previous_run
    run_step assets check_boot_assets
    run_step build build_binaries
    run_step build-disks build_disks
    run_step pack pack_source_disk
}

run_in_namespace() {
    run_step network setup_cable_bridge
    run_step imaged-server start_imaged_server
    run_step dhcp start_dhcp_server
    run_step import import_packed_archive
    run_step capture capture_source_disk
    run_step export export_images
    run_step compare compare_archives
    run_step deploy-packed deploy_packed
    run_step verify-packed verify_deployed_disk "$SOURCE_DISK" "$PACKED_DISK" e2e-packed
    run_step deploy-captured deploy_captured
    run_step verify-captured verify_deployed_disk "$SOURCE_DISK" "$CAPTURED_DISK" e2e-captured
    print_step_summary
    log "PASS: packed and captured images imported, compared and deployed with post-install scripts ($FIRMWARE firmware)"
}

if [[ -z ${E2E_IN_NAMESPACE:-} ]]; then
    prepare_outside_namespace
fi
enter_cable_namespace "$0" "$@"
run_in_namespace

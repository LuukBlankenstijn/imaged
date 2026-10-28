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
PARTITION_PACKED_MAC=52:54:00:e2:e0:04
PARTITION_CAPTURED_MAC=52:54:00:e2:e0:05
SOURCE_DISK=$OUT_DIR/source.raw
PACKED_DISK=$OUT_DIR/deploy-packed.raw
CAPTURED_DISK=$OUT_DIR/deploy-captured.raw
PARTITION_PACKED_DISK=$OUT_DIR/partition-packed.raw
PARTITION_CAPTURED_DISK=$OUT_DIR/partition-captured.raw
ROOT_FS_IMAGE=$OUT_DIR/root-fs.img
SCRIPT_DIR=$OUT_DIR/scripts
SCRIPT_NAME=e2e-hostname
PACKED_ARCHIVE=$OUT_DIR/packed.imaged
PACKED_EXPORT=$OUT_DIR/packed-export.imaged
CAPTURED_ARCHIVE=$OUT_DIR/captured.imaged
PARTITION_PACKED_ARCHIVE=$OUT_DIR/partition-packed.imaged
PARTITION_CAPTURED_ARCHIVE=$OUT_DIR/partition-captured.imaged
COMPARE_DIR=$OUT_DIR/compare
IMAGED_DATA=$OUT_DIR/imaged-data

clean_previous_run() {
    rm -rf "$IMAGED_DATA" "$OUT_DIR/verify" "$PARTITION_SNAPSHOTS" "$COMPARE_DIR" "$SCRIPT_DIR" "$STEP_LOG" \
        "$OUT_DIR"/*.log "$OUT_DIR"/*.raw "$OUT_DIR"/*.img "$OUT_DIR"/*.fd "$OUT_DIR"/*.imaged "$OUT_DIR"/*.imaged.* \
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
    for binary in bin/debugfs bin/resize2fs bin/imaged-agent; do
        grep -qxE "(\./)?$binary" <<<"$listing" ||
            fail "$initramfs has no $binary; it was built from an older tree, run build-initramfs x86_64"
    done
    log "initramfs carries bin/debugfs, bin/resize2fs and bin/imaged-agent"
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
    build_partition_target_disk "$PARTITION_PACKED_DISK"
    build_partition_target_disk "$PARTITION_CAPTURED_DISK"
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

manifest_of() {
    tar -xOf "$1" manifest.json
}

root_fs_uuid() {
    blkid -p -o value -s UUID "$ROOT_FS_IMAGE"
}

pack_root_filesystem() {
    extract_partition "$SOURCE_DISK" 2 "$ROOT_FS_IMAGE"
    local uuid input_bytes input_sha
    uuid=$(root_fs_uuid)
    input_bytes=$(stat -c %s "$ROOT_FS_IMAGE")
    input_sha=$(sha256sum <"$ROOT_FS_IMAGE")
    if ! "$IMAGED_PACK" "$ROOT_FS_IMAGE" --name e2e-partition-packed --script "$SCRIPT_DIR/$SCRIPT_NAME.sh" \
        -o "$PARTITION_PACKED_ARCHIVE" >"$OUT_DIR/imaged-pack-partition.log" 2>&1; then
        tail -n 30 "$OUT_DIR/imaged-pack-partition.log" >&2 || true
        fail "imaged-pack failed on $(relative "$ROOT_FS_IMAGE"), see $(relative "$OUT_DIR/imaged-pack-partition.log")"
    fi
    log_archive_entries "$PARTITION_PACKED_ARCHIVE"
    [[ $(sha256sum <"$ROOT_FS_IMAGE") == "$input_sha" ]] || fail "imaged-pack modified its input $(relative "$ROOT_FS_IMAGE")"

    local manifest entries
    manifest=$(manifest_of "$PARTITION_PACKED_ARCHIVE")
    jq -e --arg uuid "$uuid" --argjson input "$input_bytes" '
        .format == 3 and .kind == "partition" and (has("partition_table") | not) and .root_partition == 1
        and (.partitions | length) == 1
        and (.partitions[0] | .number == 1 and .fstype == "ext4" and .fs_uuid == $uuid and .size_bytes < $input)' \
        <<<"$manifest" >/dev/null ||
        fail "$(relative "$PARTITION_PACKED_ARCHIVE") is not a shrunk format 3 partition archive of filesystem $uuid: $manifest"
    entries=$(tar -tf "$PARTITION_PACKED_ARCHIVE")
    if grep -qx parttable.bin <<<"$entries"; then
        fail "$(relative "$PARTITION_PACKED_ARCHIVE") holds a parttable.bin"
    fi
    log "packed the ext4 filesystem $(relative "$ROOT_FS_IMAGE") ($input_bytes bytes, UUID $uuid, left unchanged) as a format 3 partition archive shrunk to $(jq .partitions[0].size_bytes <<<"$manifest") bytes"
}

start_imaged_server() {
    cat >"$OUT_DIR/imaged.toml" <<EOF
data_dir = "$IMAGED_DATA"
multicast_interface = "br0"
public_url = "http://$SERVER_IP:8080"

[listen]
machine = "$SERVER_IP:8080"
tftp = "$SERVER_IP:69"
dashboard = "machine"
EOF
    start_background imaged-server "$OUT_DIR/imaged-server.log" \
        "$IMAGED_SERVER" --config "$OUT_DIR/imaged.toml"
    wait_until 60 "imaged-server to answer on $IMAGED_URL" imaged_answers
}

assert_image() {
    local id=$1 what=$2 kind=$3 root=$4 image
    image=$(image_json "$id")
    jq -e --arg kind "$kind" --argjson root "$root" '.kind == $kind and .root_partition == $root' <<<"$image" >/dev/null ||
        fail "$what image $id is not a $kind image with root partition $root: $image"
    log "$what image $id is a $kind image with root partition $root"
}

assert_partition_image() {
    local id=$1 what=$2 size=$3 image
    assert_image "$id" "$what" partition 1
    image=$(image_json "$id")
    jq -e --argjson size "$size" --arg uuid "$(root_fs_uuid)" \
        '[.partitions[] | {partition_number, fstype, size_bytes, fs_uuid}] == [{partition_number: 1, fstype: "ext4", size_bytes: $size, fs_uuid: $uuid}]' \
        <<<"$image" >/dev/null ||
        fail "$what image $id does not hold only partition 1, ext4 filesystem $(root_fs_uuid) of $size bytes: $image"
    log "$what image $id holds partition 1: ext4 filesystem $(root_fs_uuid), $size bytes"
}

host_json() {
    api_get /api/ui/hosts | jq -c --argjson id "$1" '.[] | select(.id == $id)'
}

host_reports_partitions() {
    [[ $(host_json "$1" | jq '.partitions | length') == "$2" ]]
}

host_partuuid() {
    host_json "$1" | jq -r --argjson n "$2" '.partitions[] | select(.number == $n) | .partuuid'
}

host_partitions_summary() {
    host_json "$1" | jq -r '[.partitions[] | "p\(.number) \(.partuuid) \(.fstype // "-") label=\(.label // "-") \(.size_bytes)B"] | join(", ")'
}

disk_partuuid() {
    sfdisk --json "$1" | jq -r --argjson n "$2" '.partitiontable.partitions[$n - 1].uuid | ascii_downcase'
}

partition_bytes() {
    local start size
    read -r start size < <(partition_sectors "$1" "$2")
    echo $((size * 512))
}

attach_test_script() {
    local image_id=$1 scripts
    api_post /api/ui/images/scripts \
        "$(jq -nc --argjson image "$image_id" --argjson script "$SCRIPT_ID" '{req: {image_id: $image, script_ids: [$script]}}')" >/dev/null
    scripts=$(api_get "/api/ui/images/$image_id/scripts")
    jq -e --arg name "$SCRIPT_NAME" 'map(.name) == [$name]' <<<"$scripts" >/dev/null ||
        fail "attaching $SCRIPT_NAME to image $image_id left its scripts at: $scripts"
    log "attached library script $SCRIPT_NAME to image $image_id"
}

expect_refusal() {
    local status=$1 path=$2 body=$3 response code
    response=$(curl -sS --noproxy '*' --max-time 10 -w '\n%{http_code}' -H 'content-type: application/json' \
        -d "$body" "$IMAGED_URL$path") || fail "request $path failed: $response"
    code=${response##*$'\n'}
    response=${response%$'\n'*}
    [[ $code == "$status" ]] || fail "$path answered $code instead of $status to $body: $response"
    log "$path refused with $code: $response"
}

import_packed_archive() {
    PACKED_IMAGE_ID=$(api_upload /api/ui/images/import "$PACKED_ARCHIVE" | jq -r .id)
    log "imported $(relative "$PACKED_ARCHIVE") as image $PACKED_IMAGE_ID"
    wait_until 120 "image $PACKED_IMAGE_ID to become ready" image_ready "$PACKED_IMAGE_ID"
    log "image $PACKED_IMAGE_ID ready: $(image_json "$PACKED_IMAGE_ID")"
    assert_image "$PACKED_IMAGE_ID" packed disk 2

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

import_partition_archive() {
    PARTITION_PACKED_IMAGE_ID=$(api_upload /api/ui/images/import "$PARTITION_PACKED_ARCHIVE" | jq -r .id)
    log "imported $(relative "$PARTITION_PACKED_ARCHIVE") as image $PARTITION_PACKED_IMAGE_ID"
    wait_until 120 "image $PARTITION_PACKED_IMAGE_ID to become ready" image_ready "$PARTITION_PACKED_IMAGE_ID"
    log "image $PARTITION_PACKED_IMAGE_ID ready: $(image_json "$PARTITION_PACKED_IMAGE_ID")"
    assert_partition_image "$PARTITION_PACKED_IMAGE_ID" "packed partition" \
        "$(manifest_of "$PARTITION_PACKED_ARCHIVE" | jq .partitions[0].size_bytes)"
    local scripts
    scripts=$(api_get "/api/ui/images/$PARTITION_PACKED_IMAGE_ID/scripts")
    jq -e --argjson id "$SCRIPT_ID" 'map(.id) == [$id]' <<<"$scripts" >/dev/null ||
        fail "packed partition image $PARTITION_PACKED_IMAGE_ID does not reuse library script $SCRIPT_ID: $scripts"
    log "packed partition image $PARTITION_PACKED_IMAGE_ID reuses library script $SCRIPT_NAME ($SCRIPT_ID)"
}

capture_source_disk() {
    start_network_boot_vm capture "$CAPTURE_MAC" "$SOURCE_DISK" tap-capture
    wait_until 300 "the capture VM's agent to register" host_registered "$CAPTURE_MAC"
    CAPTURE_HOST_ID=$(host_id_for_mac "$CAPTURE_MAC")
    log "capture VM registered as host $CAPTURE_HOST_ID: $(host_json "$CAPTURE_HOST_ID")"

    CAPTURED_IMAGE_ID=$(api_post /api/ui/images/create \
        "$(jq -nc --argjson host "$CAPTURE_HOST_ID" '{req: {name: "e2e-captured", host_id: $host}}')" | jq -r .image.id)
    log "created image $CAPTURED_IMAGE_ID, capture task dispatched"
    wait_until 600 "image $CAPTURED_IMAGE_ID to become ready" image_ready "$CAPTURED_IMAGE_ID"
    log "image $CAPTURED_IMAGE_ID ready: $(image_field "$CAPTURED_IMAGE_ID" '[.partitions[] | "p\(.partition_number) \(.fstype) \(.size_bytes)B"] | join(", ")')"

    local task_id
    task_id=$(api_get "/api/ui/tasks?type=capture&limit=200" | jq -r --argjson image "$CAPTURED_IMAGE_ID" '.tasks[] | select(.image_id == $image and .type == "capture") | .id')
    wait_until 60 "capture task $task_id to finish" task_done "$task_id"
    assert_image "$CAPTURED_IMAGE_ID" captured disk 2
    attach_test_script "$CAPTURED_IMAGE_ID"
}

capture_source_partition() {
    wait_until 60 "the capture VM to report 3 partitions" host_reports_partitions "$CAPTURE_HOST_ID" 3
    log "capture VM reports partitions: $(host_partitions_summary "$CAPTURE_HOST_ID")"
    local partuuid
    partuuid=$(host_partuuid "$CAPTURE_HOST_ID" 2)
    [[ $partuuid == "$(disk_partuuid "$SOURCE_DISK" 2)" ]] ||
        fail "capture VM reports partition 2 as '$partuuid', the source disk has $(disk_partuuid "$SOURCE_DISK" 2)"

    local created task_id
    created=$(api_post /api/ui/images/create "$(jq -nc --argjson host "$CAPTURE_HOST_ID" --arg partition "$partuuid" \
        '{req: {name: "e2e-partition-captured", host_id: $host, partition: $partition}}')")
    PARTITION_CAPTURED_IMAGE_ID=$(jq -r .image.id <<<"$created")
    task_id=$(jq -r .task.id <<<"$created")
    jq -e --arg partuuid "$partuuid" '.image.kind == "partition" and .task.type == "capture" and .task.hosts[0].partuuid == $partuuid' \
        <<<"$created" >/dev/null || fail "creating a capture of partition $partuuid returned: $created"
    log "created partition image $PARTITION_CAPTURED_IMAGE_ID, capture task $task_id of partition $partuuid dispatched"
    wait_until 600 "image $PARTITION_CAPTURED_IMAGE_ID to become ready" image_ready "$PARTITION_CAPTURED_IMAGE_ID"
    wait_until 60 "capture task $task_id to finish" task_done "$task_id"
    stop_process capture
    assert_partition_image "$PARTITION_CAPTURED_IMAGE_ID" "captured partition" "$(partition_bytes "$SOURCE_DISK" 2)"
    attach_test_script "$PARTITION_CAPTURED_IMAGE_ID"
}

export_images() {
    api_download "/api/ui/images/$CAPTURED_IMAGE_ID/export" "$CAPTURED_ARCHIVE"
    log_archive_entries "$CAPTURED_ARCHIVE"
    api_download "/api/ui/images/$PACKED_IMAGE_ID/export" "$PACKED_EXPORT"
    log_archive_entries "$PACKED_EXPORT"
    api_download "/api/ui/images/$PARTITION_CAPTURED_IMAGE_ID/export" "$PARTITION_CAPTURED_ARCHIVE"
    log_archive_entries "$PARTITION_CAPTURED_ARCHIVE"

    local archive
    for archive in "$PACKED_ARCHIVE" "$CAPTURED_ARCHIVE" "$PACKED_EXPORT"; do
        [[ $(manifest_of "$archive" | jq .format) == 2 ]] || fail "disk image archive $(relative "$archive") is not format 2"
    done
    log "disk image archives are format 2"
    local manifest entries
    manifest=$(manifest_of "$PARTITION_CAPTURED_ARCHIVE")
    jq -e --arg uuid "$(root_fs_uuid)" '.format == 3 and .kind == "partition" and (has("partition_table") | not)
        and .root_partition == 1 and [.partitions[] | {number, fs_uuid}] == [{number: 1, fs_uuid: $uuid}]' \
        <<<"$manifest" >/dev/null || fail "the captured partition export is not a format 3 partition archive: $manifest"
    entries=$(tar -tf "$PARTITION_CAPTURED_ARCHIVE")
    if grep -qx parttable.bin <<<"$entries"; then
        fail "the captured partition export holds a parttable.bin"
    fi
    log "the captured partition image exports as format 3 with kind partition and no partition table"
}

manifest_structure() {
    tar -xOf "$1" manifest.json | jq -S '[paths | map(if type == "number" then 0 else . end)] | unique'
}

compare_archives() {
    local captured=$1 packed=$2
    mkdir -p "$COMPARE_DIR"
    local archive name names=()
    for archive in "$captured" "$packed"; do
        name=$(basename "$archive" .imaged)
        names+=("$name")
        tar -tf "$archive" >"$COMPARE_DIR/$name.entries"
        tar -xOf "$archive" manifest.json | jq -S . >"$COMPARE_DIR/$name.manifest.json"
        manifest_structure "$archive" >"$COMPARE_DIR/$name.structure.json"
        log "$name entries: $(paste -sd' ' "$COMPARE_DIR/$name.entries")"
        log "$name manifest: $(jq -c . "$COMPARE_DIR/$name.manifest.json")"
    done
    diff "$COMPARE_DIR/${names[0]}.entries" "$COMPARE_DIR/${names[1]}.entries" >&2 ||
        fail "the captured export ${names[0]} and the packed archive ${names[1]} have different entries"
    diff "$COMPARE_DIR/${names[0]}.structure.json" "$COMPARE_DIR/${names[1]}.structure.json" >&2 ||
        fail "the ${names[0]} and ${names[1]} manifests have different key structures"
    log "the captured export ${names[0]} and the packed archive ${names[1]} have the same entries and manifest structure"
}

deploy_finished() {
    task_done "$DEPLOY_TASK_ID" && return 0
    running "$DEPLOY_VM" && return 1
    task_done "$DEPLOY_TASK_ID" || fail "$DEPLOY_VM VM exited while task $DEPLOY_TASK_ID is $(task_field "$DEPLOY_TASK_ID" .state)"
}

restore_image() {
    local via=$1 name=$2 mac=$3 disk=$4 tap=$5 image_id=$6 hostname=$7 partition=${8:-}
    start_network_boot_vm "$name" "$mac" "$disk" "$tap"
    wait_until 300 "the $name VM's agent to register" host_registered "$mac"
    local host_id target=
    host_id=$(host_id_for_mac "$mac")
    log "$name VM registered as host $host_id"
    if [[ -n $partition ]]; then
        wait_until 60 "the $name VM to report 3 partitions" host_reports_partitions "$host_id" 3
        log "$name VM reports partitions: $(host_partitions_summary "$host_id")"
        target=$(host_partuuid "$host_id" "$partition")
        [[ $target == "$(disk_partuuid "$disk" "$partition")" ]] ||
            fail "$name VM reports partition $partition as '$target', the disk has $(disk_partuuid "$disk" "$partition")"
    fi

    local inputs request path
    inputs=$(jq -nc --argjson host "$host_id" --arg hostname "$hostname" \
        '{shared: {}, per_machine: {($host | tostring): {hostname: {value: $hostname}}}}')
    case $via in
        deploy)
            path=/api/ui/hosts/deploy
            request=$(jq -nc --argjson host "$host_id" --argjson image "$image_id" --argjson inputs "$inputs" --arg target "$target" \
                '{req: ({id: $host, image_id: $image, inputs: $inputs} + if $target == "" then {} else {partition: $target} end)}')
            ;;
        multicast)
            path=/api/ui/groups/multicast
            request=$(jq -nc --argjson host "$host_id" --argjson image "$image_id" --argjson inputs "$inputs" --arg target "$target" \
                '{req: ({host_ids: [$host], image_id: $image, inputs: $inputs} + if $target == "" then {} else {partitions: {($host | tostring): $target}} end)}')
            ;;
    esac
    if [[ -n $target ]]; then
        expect_partition_refusals "$path" "$request" "$host_id"
    fi

    local task
    DEPLOY_VM=$name
    task=$(api_post "$path" "$request")
    DEPLOY_TASK_ID=$(jq -r .id <<<"$task")
    jq -e --arg via "$via" --arg target "$target" '.type == $via and (.hosts[0].partuuid // "") == $target' <<<"$task" >/dev/null ||
        fail "$path created an unexpected task: $task"
    log "$via task $DEPLOY_TASK_ID dispatched: image $image_id to host $host_id${target:+ partition $target} with hostname $hostname"
    unwatch "$name"
    wait_until 600 "$via task $DEPLOY_TASK_ID to finish" deploy_finished

    local runs
    runs=$(api_get "/api/ui/tasks/$DEPLOY_TASK_ID/hosts/$host_id/scripts")
    log "script runs: $(jq -c 'map({script_name, exit_code, output})' <<<"$runs")"
    jq -e --arg name "$SCRIPT_NAME" 'map(select(.script_name == $name and .exit_code == 0)) | length == 1' <<<"$runs" >/dev/null ||
        fail "$via task $DEPLOY_TASK_ID did not run $SCRIPT_NAME successfully: $runs"

    log "$via task $DEPLOY_TASK_ID done, waiting for the VM to reboot"
    wait_for_exit "$name" 120
}

expect_partition_refusals() {
    local path=$1 request=$2 host_id=$3
    expect_refusal 400 "$path" "$(jq -c 'del(.req.partition, .req.partitions)' <<<"$request")"
    expect_refusal 400 "$path" "$(jq -c --argjson image "$PACKED_IMAGE_ID" '.req.image_id = $image' <<<"$request")"
    local esp
    esp=$(host_partuuid "$host_id" 1)
    expect_refusal 412 "$path" "$(jq -c --argjson image "$PARTITION_CAPTURED_IMAGE_ID" --arg esp "$esp" --arg host "$host_id" \
        '.req.image_id = $image | if .req.partition then .req.partition = $esp else .req.partitions[$host] = $esp end' <<<"$request")"
}

deploy_packed() {
    restore_image deploy deploy-packed "$PACKED_MAC" "$PACKED_DISK" tap-packed "$PACKED_IMAGE_ID" e2e-packed
}

deploy_captured() {
    restore_image deploy deploy-captured "$CAPTURED_MAC" "$CAPTURED_DISK" tap-captured "$CAPTURED_IMAGE_ID" e2e-captured
}

deploy_partition_packed() {
    restore_image deploy partition-packed "$PARTITION_PACKED_MAC" "$PARTITION_PACKED_DISK" tap-ppacked \
        "$PARTITION_PACKED_IMAGE_ID" e2e-partition-packed 2
}

multicast_partition_captured() {
    restore_image multicast partition-captured "$PARTITION_CAPTURED_MAC" "$PARTITION_CAPTURED_DISK" tap-pcaptured \
        "$PARTITION_CAPTURED_IMAGE_ID" e2e-partition-captured 2
}

prepare_outside_namespace() {
    require_commands cargo cc unshare ip qemu-system-x86_64 dnsmasq curl jq sgdisk sfdisk blkid mke2fs e2fsck debugfs \
        dumpe2fs resize2fs mkfs.vfat fsck.fat mcopy mkswap partclone.extfs partclone.vfat xz tar
    [[ -x ${E2E_BUSYBOX:-} ]] || fail "E2E_BUSYBOX must point at a static busybox binary (run inside: nix develop $REPO_DIR#e2e)"
    [[ -r /dev/kvm && -w /dev/kvm ]] || fail "/dev/kvm is not accessible"
    mkdir -p "$OUT_DIR"
    clean_previous_run
    run_step assets check_boot_assets
    run_step build build_binaries
    run_step build-disks build_disks
    run_step pack pack_source_disk
    run_step pack-partition pack_root_filesystem
}

run_in_namespace() {
    run_step network setup_cable_bridge
    run_step imaged-server start_imaged_server
    run_step dhcp start_dhcp_server
    run_step import import_packed_archive
    run_step import-partition import_partition_archive
    run_step capture capture_source_disk
    run_step capture-partition capture_source_partition
    run_step export export_images
    run_step compare compare_archives "$CAPTURED_ARCHIVE" "$PACKED_ARCHIVE"
    run_step compare-partition compare_archives "$PARTITION_CAPTURED_ARCHIVE" "$PARTITION_PACKED_ARCHIVE"
    run_step deploy-packed deploy_packed
    run_step verify-packed verify_deployed_disk "$SOURCE_DISK" "$PACKED_DISK" e2e-packed
    run_step deploy-captured deploy_captured
    run_step verify-captured verify_deployed_disk "$SOURCE_DISK" "$CAPTURED_DISK" e2e-captured
    run_step deploy-partition-packed deploy_partition_packed
    run_step verify-partition-packed verify_partition_deploy "$PARTITION_PACKED_DISK" e2e-partition-packed \
        "$(root_fs_uuid)" "$(image_field "$PARTITION_PACKED_IMAGE_ID" .partitions[0].size_bytes)"
    run_step multicast-partition-captured multicast_partition_captured
    run_step verify-partition-captured verify_partition_deploy "$PARTITION_CAPTURED_DISK" e2e-partition-captured \
        "$(root_fs_uuid)" "$(image_field "$PARTITION_CAPTURED_IMAGE_ID" .partitions[0].size_bytes)"
    print_step_summary
    log "PASS: packed and captured disk and partition images imported, compared and deployed with post-install scripts ($FIRMWARE firmware)"
}

if [[ -z ${E2E_IN_NAMESPACE:-} ]]; then
    prepare_outside_namespace
fi
enter_cable_namespace "$0" "$@"
run_in_namespace

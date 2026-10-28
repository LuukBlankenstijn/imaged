#!/usr/bin/env bash
# usage: nix develop .#e2e -c e2e/nixos.sh  (env: E2E_NIXOS_CASES="sdboot:uefi grub-bios:bios ...", E2E_OUT, CARGO_TARGET_DIR, IMAGED_BOOT_ASSETS)
export E2E_OUT=${E2E_OUT:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/out/nixos}
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$E2E_DIR/disk.sh"

IMAGED_BOOT_ASSETS=$(realpath -m "${IMAGED_BOOT_ASSETS:-$REPO_DIR/assets}")
export IMAGED_BOOT_ASSETS
TARGET_DIR=$(cd "$REPO_DIR" && realpath -m "${CARGO_TARGET_DIR:-target}")
IMAGED_SERVER=$TARGET_DIR/x86_64-unknown-linux-gnu/debug/imaged-server
DISKO=github:nix-community/disko/725ea35e410ad83be4931d1bff7e090eacaf3563
read -ra CASES <<<"${E2E_NIXOS_CASES:-sdboot:uefi grub-bios:bios grub-hybrid:bios grub-hybrid:uefi disko:uefi disko-grub:bios partition:uefi partition-grub-bios:bios}"
DISK_SIZE=8G
MIN_ROOT_BYTES=$((6 * 1024 * 1024 * 1024))
AGE_KEY=$'# created: 2026-10-13\n# public key: age1e2e\nAGE-SECRET-KEY-1E2EE2EE2E\n'

configurations() {
    printf 'import %s/e2e/nixos/configurations.nix { imaged = builtins.getFlake "git+file://%s"; disko = builtins.getFlake "%s"; }' \
        "$REPO_DIR" "$REPO_DIR" "$DISKO"
}

archive_of() {
    find -L "$OUT_DIR/archive-$1" -name '*.imaged' | head -1
}

sha() {
    printf '%s' "$1" | sha256sum | cut -d' ' -f1
}

app_token() {
    printf 'app token for %s\nsecond line\n' "$1"
}

check_invalid_configurations() {
    local messages message output
    messages=$(nix eval --impure --json --expr "builtins.attrNames ($(configurations)).invalid" | jq -r '.[]')
    while read -r message; do
        if output=$(nix eval --impure --raw --expr "($(configurations)).invalid.\"$message\".config.system.build.images.imaged.drvPath" 2>&1); then
            fail "the invalid configuration '$message' evaluated: $output"
        fi
        grep -qF -- "$message" <<<"$output" || fail "the invalid configuration '$message' failed with another error: $(tail -n 5 <<<"$output")"
        log "rejected at evaluation: $message"
    done <<<"$messages"
}

build_archives() {
    local configs=() config
    for spec in "${CASES[@]}"; do
        config=${spec%%:*}
        [[ " ${configs[*]} " == *" $config "* ]] || configs+=("$config")
    done
    for config in "${configs[@]}"; do
        rm -f "$OUT_DIR/archive-$config"
        nice nix build --impure --max-jobs 2 --cores 2 --out-link "$OUT_DIR/archive-$config" \
            --expr "($(configurations)).$config.config.system.build.images.imaged" \
            >"$OUT_DIR/build-$config.log" 2>&1 ||
            fail "building the $config image failed: $(tail -n 20 "$OUT_DIR/build-$config.log")"
        log "built $(archive_of "$config") ($(stat -L -c %s "$(archive_of "$config")") bytes)"
    done
}

build_server() {
    mkdir -p "$REPO_DIR/crates/web/assets"
    [[ -f $REPO_DIR/crates/web/assets/tailwind.css ]] || : >"$REPO_DIR/crates/web/assets/tailwind.css"
    (cd "$REPO_DIR" && SQLX_OFFLINE=true cargo build --locked --target x86_64-unknown-linux-gnu -p imaged-server --features server)
    mkdir -p "$(dirname "$IMAGED_SERVER")/public"
}

start_server() {
    printf '%s' "$AGE_KEY" >"$OUT_DIR/age-key"
    cat >"$OUT_DIR/imaged.toml" <<EOF
data_dir = "$OUT_DIR/imaged-data"
multicast_interface = "br0"
public_url = "$IMAGED_URL"

[listen]
machine = "$SERVER_IP:8080"
dashboard = "machine"
tftp = "$SERVER_IP:69"

[script_variables]
AGE_KEY_file_contents = "age-key"
EOF
    start_background imaged-server "$OUT_DIR/imaged-server.log" "$IMAGED_SERVER" --config "$OUT_DIR/imaged.toml"
    wait_until 60 "imaged-server to answer on $IMAGED_URL" imaged_answers
    start_dhcp_server
}

declare -A IMAGE_IDS=()

import_image() {
    local config=$1
    [[ -n ${IMAGE_IDS[$config]:-} ]] && return
    IMAGE_IDS[$config]=$(api_upload /api/ui/images/import "$(archive_of "$config")" | jq -r .id)
    wait_until 300 "image ${IMAGE_IDS[$config]} to become ready" image_ready "${IMAGE_IDS[$config]}"
    local kinds
    kinds=$(api_get "/api/ui/images/${IMAGE_IDS[$config]}/scripts" | jq -c '[.[].inputs[] | "\(.name):\(.kind):\(.per_machine)"] | sort')
    local script
    script=$(api_get "/api/ui/images/${IMAGE_IDS[$config]}/scripts" | jq -r ".[].name")
    [[ $script == imaged-nixos-* ]] || fail "image $config runs script $script"
    [[ $kinds == '["age_key:secret:false","app_token:secret:true","nested:secret:true"]' ]] ||
        fail "image $config declares unexpected inputs: $kinds"
    local kind expected=disk
    [[ $config == partition* ]] && expected=partition
    kind=$(image_field "${IMAGE_IDS[$config]}" .kind)
    [[ $kind == "$expected" ]] || fail "image $config is a $kind image, expected $expected: $(image_json "${IMAGE_IDS[$config]}")"
    log "imported $config as $kind image ${IMAGE_IDS[$config]} with secret inputs $kinds"
}

deploy_finished() {
    task_done "$DEPLOY_TASK_ID" && return 0
    running "$DEPLOY_VM" && return 1
    task_done "$DEPLOY_TASK_ID" || fail "$DEPLOY_VM exited while task $DEPLOY_TASK_ID is $(task_field "$DEPLOY_TASK_ID" .state)"
}

deploy_case() {
    local name=$1 config=$2 mac=$3 disk=$4 tap=$5 partition=${6:-}
    start_network_boot_vm "$name" "$mac" "$disk" "$tap"
    wait_until 300 "the $name agent to register" host_registered "$mac"
    local host_id request target=
    host_id=$(host_id_for_mac "$mac")
    if [[ -n $partition ]]; then
        wait_until 60 "the $name agent to report its partitions" host_reports_partition "$host_id" "$partition"
        target=$(api_get /api/ui/hosts | jq -r --argjson id "$host_id" --argjson n "$partition" \
            '.[] | select(.id == $id) | .partitions[] | select(.number == $n) | .partuuid')
        log "deploying $config into partition $partition ($target) of $name"
    fi
    request=$(jq -nc --argjson host "$host_id" --argjson image "${IMAGE_IDS[$config]}" \
        --arg token "$(app_token "$name")" --arg nested "$(app_token "$name-nested")" --arg target "$target" \
        '{req: ({id: $host, image_id: $image, inputs: {
            shared: {age_key: {variable: "AGE_KEY"}},
            per_machine: {($host | tostring): {app_token: {value: $token}, nested: {value: $nested}}}}}
            + if $target == "" then {} else {partition: $target} end)}')
    DEPLOY_VM=$name
    DEPLOY_TASK_ID=$(api_post /api/ui/hosts/deploy "$request" | jq -r .id)
    unwatch "$name"
    wait_until 900 "deploy task $DEPLOY_TASK_ID to finish" deploy_finished
    api_get "/api/ui/tasks/$DEPLOY_TASK_ID/hosts/$host_id/scripts" | jq -r '.[] | .output' >"$OUT_DIR/$name.script.log"
    wait_for_exit "$name" 120
    log "deployed $config to $name; post-install output in $(relative "$OUT_DIR/$name.script.log")"
}

host_reports_partition() {
    api_get /api/ui/hosts | jq -e --argjson id "$1" --argjson n "$2" \
        '.[] | select(.id == $id) | .partitions | any(.number == $n)' >/dev/null
}

prepartition_disk() {
    local disk=$1 work
    work=$(mktemp -d)
    sgdisk --clear \
        --new=1:1MiB:+1MiB --typecode=1:ef02 --change-name=1:bios \
        --new=2:0:+256MiB --typecode=2:ef00 --change-name=2:ESP \
        --new=3:0:0 --typecode=3:8300 --change-name=3:root \
        "$disk" >/dev/null
    mkfs.vfat -F 32 -n ESP -C "$work/esp.img" "$(partition_kib "$disk" 2)" >/dev/null
    write_partition "$disk" 2 "$work/esp.img"
    rm -rf "$work"
    partition_layout "$disk" >"$OUT_DIR/$(basename "$disk" .raw).layout.json"
    log "partitioned $(relative "$disk"): p1 BIOS boot, p2 vfat ESP labelled ESP, p3 empty root"
}

partition_label() {
    local start size
    read -r start size < <(partition_sectors "$1" "$2")
    blkid -p -O $((start * 512)) -o value -s LABEL "$1"
}

verify_partition_table() {
    local name=$1 disk=$2
    diff "$OUT_DIR/$name.layout.json" <(partition_layout "$disk") >&2 ||
        fail "$name: the partition table changed after the partition deploy and boot"
    [[ $(partition_label "$disk" 2) == ESP && $(partition_label "$disk" 3) == nixos ]] ||
        fail "$name: p2 is labelled '$(partition_label "$disk" 2)' and p3 '$(partition_label "$disk" 3)', expected ESP and nixos"
    log "$name: partition table unchanged by the deploy and boot, p2 still the ESP, p3 the nixos root"
}

booted() {
    grep -aq 'IMAGED-E2E DONE' "$OUT_DIR/$1.boot.serial.log" 2>/dev/null
}

boot_case() {
    local name=$1 disk=$2
    local firmware=()
    if [[ $FIRMWARE == uefi ]]; then
        local share
        share=$(dirname "$(command -v qemu-system-x86_64)")/../share/qemu
        firmware=(-drive "if=pflash,format=raw,readonly=on,file=$share/edk2-x86_64-code.fd"
            -drive "if=pflash,format=raw,file=$OUT_DIR/$name.vars.fd")
    fi
    VM_NAMES+=("$name.boot")
    start_background "$name.boot" "$OUT_DIR/$name.boot.qemu.log" \
        qemu-system-x86_64 -name "$name-boot" -machine q35,accel=kvm -cpu host -smp 2 -m 1G -nodefaults -display none \
        "${firmware[@]}" -serial "file:$OUT_DIR/$name.boot.serial.log" -nic none \
        -drive "file=$disk,format=raw,if=none,id=disk" -device virtio-blk-pci,drive=disk,bootindex=1
    wait_until 300 "the deployed $name disk to boot and report" booted "$name"
    stop_process "$name.boot"
}

report() {
    tr -d '\r' <"$OUT_DIR/$1.boot.serial.log" | grep -ao 'IMAGED-E2E .*' | sed 's/^IMAGED-E2E //'
}

expect_file() {
    local lines=$1 path=$2 owner=$3 mode=$4 content=$5
    local expected
    expected="file $path $owner $mode $(sha "$content") readable"
    grep -qxF -- "$expected" <<<"$lines" || fail "expected '$expected', the deployed system reported: $(grep "file $path " <<<"$lines")"
}

verify_case() {
    local name=$1 config=$2
    local lines
    lines=$(report "$name")
    expect_file "$lines" /var/lib/app/token app:app 440 "$(app_token "$name")"
    expect_file "$lines" /var/lib/nested-app/keys/deep/token app:app 400 "$(app_token "$name-nested")"
    expect_file "$lines" /var/lib/sops-nix/key.txt root:root 400 "$AGE_KEY"
    local root
    root=$(sed -n 's/^root //p' <<<"$lines")
    ((root > MIN_ROOT_BYTES)) || fail "$name: the root filesystem is $root bytes, it did not grow to fill the $DISK_SIZE disk"
    local efi
    efi=$(sed -n 's/^efi //p' <<<"$lines")
    [[ $efi == $([[ $FIRMWARE == uefi ]] && echo yes || echo no) ]] || fail "$name booted with efi=$efi on $FIRMWARE firmware"
    if [[ $config == disko* ]]; then
        grep -qE '^swap [0-9a-f-]{36} 1$' <<<"$lines" || fail "$name: the disko swap partition is not active: $(grep '^swap' <<<"$lines")"
    fi
    log "$name ($config, $FIRMWARE): secrets written with their owners and modes, root grew to $root bytes$([[ $config == disko* ]] && echo ", swap active")"
}

import_all_images() {
    local spec
    for spec in "${CASES[@]}"; do
        import_image "${spec%%:*}"
    done
}

run_case() {
    local index=$1 spec=$2
    local config=${spec%%:*}
    FIRMWARE=${spec##*:}
    local name=$config-$FIRMWARE
    local mac
    mac=$(printf '52:54:00:e2:e1:%02x' "$index")
    local disk=$OUT_DIR/$name.raw
    rm -f "$disk"
    truncate -s "$DISK_SIZE" "$disk"
    local partition=
    if [[ $config == partition* ]]; then
        prepartition_disk "$disk"
        partition=3
    fi
    import_image "$config"
    deploy_case "$name" "$config" "$mac" "$disk" "tap-$index" "$partition"
    boot_case "$name" "$disk"
    verify_case "$name" "$config"
    if [[ -n $partition ]]; then
        verify_partition_table "$name" "$disk"
    fi
}

if [[ -z ${E2E_IN_NAMESPACE:-} ]]; then
    require_commands nix cargo unshare ip qemu-system-x86_64 dnsmasq curl jq sgdisk sfdisk blkid mkfs.vfat
    [[ -r /dev/kvm && -w /dev/kvm ]] || fail "/dev/kvm is not accessible"
    mkdir -p "$OUT_DIR"
    rm -rf "$OUT_DIR/imaged-data" "$OUT_DIR"/*.raw "$OUT_DIR"/*.log "$OUT_DIR"/*.fd "$OUT_DIR"/*.layout.json "$OUT_DIR/steps.txt"
    run_step invalid check_invalid_configurations
    run_step build-images build_archives
    run_step build-server build_server
fi
enter_cable_namespace "$0" "$@"
run_step network setup_cable_bridge
run_step server start_server
run_step import import_all_images
index=0
for spec in "${CASES[@]}"; do
    index=$((index + 1))
    run_step "$spec" run_case "$index" "$spec"
done
print_step_summary
log "PASS: NixOS images built, rejected where invalid, deployed and booted: ${CASES[*]}"

#!/usr/bin/env bash
# usage: nix develop .#e2e -c e2e/boot.sh  (env: E2E_BOOT_CASES="extra-initrds:bios profile:uefi arm64:uefi ...", E2E_OUT, CARGO_TARGET_DIR, IMAGED_BOOT_ASSETS)
export E2E_OUT=${E2E_OUT:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/out/boot}
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

IMAGED_BOOT_ASSETS=$(realpath -m "${IMAGED_BOOT_ASSETS:-$REPO_DIR/assets}")
export IMAGED_BOOT_ASSETS
TARGET_DIR=$(cd "$REPO_DIR" && realpath -m "${CARGO_TARGET_DIR:-target}")
IMAGED_SERVER=$TARGET_DIR/x86_64-unknown-linux-gnu/debug/imaged-server
FIXTURES=$OUT_DIR/fixtures
read -ra CASES <<<"${E2E_BOOT_CASES:-extra-initrds:bios extra-initrds:uefi profile:bios profile:uefi arm64:uefi}"
INIT_MARKER="imaged-e2e: /init from an appended uncompressed archive"
QEMU_PRODUCT="Standard PC (Q35 + ICH9, 2009)"

flake() {
    printf 'builtins.getFlake "git+file://%s"' "$REPO_DIR"
}

build_server() {
    mkdir -p "$REPO_DIR/crates/web/assets"
    [[ -f $REPO_DIR/crates/web/assets/tailwind.css ]] || : >"$REPO_DIR/crates/web/assets/tailwind.css"
    (cd "$REPO_DIR" && SQLX_OFFLINE=true cargo build --locked --target x86_64-unknown-linux-gnu -p imaged-server --features server)
    mkdir -p "$(dirname "$IMAGED_SERVER")/public"
}

build_display_archives() {
    local package
    for package in display-intel display-nvidia display-nvidia-arm64; do
        nix build --out-link "$FIXTURES/$package" "git+file://$REPO_DIR#$package" ||
            fail "building $package failed"
        log "built $package ($(stat -L -c %s "$FIXTURES/$package") bytes)"
    done
}

build_custom_kernel() {
    nix build --impure --out-link "$FIXTURES/lts" --expr "
        let
          imaged = $(flake);
          pkgs = imaged.inputs.nixpkgs.legacyPackages.x86_64-linux;
          kernel = pkgs.linuxPackages.kernel;
        in
        pkgs.linkFarm \"imaged-e2e-lts\" {
          bzImage = \"\${kernel}/bzImage\";
          \"modules.cpio.xz\" = imaged.lib.x86_64-linux.modulesInitrd {
            arch = \"x86_64\";
            name = \"imaged-e2e-lts-modules\";
            inherit kernel;
            modules = [ \"drivers/(net/virtio_net|block/virtio_blk|virtio/)\" \"fs/(ext4|fat|btrfs|nls|efivarfs)/\" \"net/packet/\" ];
          };
        }" || fail "building the LTS kernel and its modules archive failed"
    nix eval --impure --raw --expr "($(flake)).inputs.nixpkgs.legacyPackages.x86_64-linux.linuxPackages.kernel.modDirVersion" \
        >"$FIXTURES/lts-version"
    log "built linux $(cat "$FIXTURES/lts-version") with a modules archive of $(stat -L -c %s "$FIXTURES/lts/modules.cpio.xz") bytes"
}

build_marker_archives() {
    local work=$FIXTURES/work
    mkdir -p "$work/initramfs" "$work/init" "$work/padding/e2e"
    (cd "$work/initramfs" && xz -dc "$IMAGED_BOOT_ASSETS/x86_64/initramfs.cpio.xz" | cpio -id --quiet)
    {
        head -n 1 "$work/initramfs/init"
        printf "echo '%s'\n" "$INIT_MARKER"
        tail -n +2 "$work/initramfs/init"
    } >"$work/init/init"
    chmod 0755 "$work/init/init"
    (cd "$work/init" && printf 'init\n' | cpio -o -H newc --quiet) >"$FIXTURES/init.cpio"
    local content=odd
    while :; do
        printf '%s' "$content" >"$work/padding/e2e/padding"
        (cd "$work/padding" && find . | cpio -o -H newc --quiet | gzip -9n) >"$FIXTURES/padding.cpio.gz"
        if (($(stat -c %s "$FIXTURES/padding.cpio.gz") % 4)); then
            break
        fi
        content+=d
    done
    rm -rf "$work"
    log "built an uncompressed archive replacing /init and a $(stat -c %s "$FIXTURES/padding.cpio.gz") byte gzip archive before it"
}

build_fixtures() {
    rm -rf "$FIXTURES"
    mkdir -p "$FIXTURES"
    build_display_archives
    build_custom_kernel
    build_marker_archives
}

padded_concat() {
    local out=$1 part size
    shift
    : >"$out"
    for part in "$@"; do
        cat "$part" >>"$out"
        size=$(stat -L -c %s "$part")
        head -c $(((4 - size % 4) % 4)) /dev/zero >>"$out"
    done
}

start_case_server() {
    local name=$1 boot=$2
    cat >"$OUT_DIR/$name.toml" <<EOF
data_dir = "$OUT_DIR/$name-data"
multicast_interface = "br0"
public_url = "$IMAGED_URL"

[listen]
machine = "$SERVER_IP:8080"
dashboard = "machine"
tftp = "$SERVER_IP:69"

$boot
EOF
    start_background imaged-server "$OUT_DIR/$name.server.log" "$IMAGED_SERVER" --config "$OUT_DIR/$name.toml"
    wait_until 60 "imaged-server to answer on $IMAGED_URL" imaged_answers
}

assert_served() {
    local name=$1 profile=$2 kernel=$3 initramfs=$4 served
    served=$OUT_DIR/$name.served
    curl -sSf --noproxy '*' -o "$served.vmlinuz" "$IMAGED_URL/boot/profile/$profile/vmlinuz" || fail "$name: GET /boot/profile/$profile/vmlinuz failed"
    cmp -s "$served.vmlinuz" "$kernel" || fail "$name: /boot/profile/$profile/vmlinuz is not $kernel"
    curl -sSf --noproxy '*' -o "$served.http" "$IMAGED_URL/boot/profile/$profile/initramfs.cpio.xz" ||
        fail "$name: GET /boot/profile/$profile/initramfs.cpio.xz failed"
    cmp -s "$served.http" "$initramfs" || fail "$name: the initramfs served over HTTP differs from the expected parts"
    curl -sSf --tftp-blksize 1468 -o "$served.tftp" "tftp://$SERVER_IP/profile/$profile/initramfs.cpio.xz" ||
        fail "$name: TFTP profile/$profile/initramfs.cpio.xz failed"
    cmp -s "$served.tftp" "$initramfs" || fail "$name: the initramfs served over TFTP differs from the expected parts"
    log "$name: HTTP and TFTP serve profile $profile's kernel and its $(stat -c %s "$initramfs") byte initramfs"
    rm -f "$served".*
}

start_arm64_vm() {
    local name=$1 mac=$2 disk=$3 tap=$4 share
    add_cable_tap "$tap"
    share=$(dirname "$(command -v qemu-system-aarch64)")/../share/qemu
    cp --no-preserve=mode "$share/edk2-arm-vars.fd" "$OUT_DIR/$name.vars.fd"
    truncate -s 64M "$OUT_DIR/$name.vars.fd"
    VM_NAMES+=("$name")
    start_background "$name" "$OUT_DIR/$name.qemu.log" \
        qemu-system-aarch64 \
        -name "$name" \
        -machine virt \
        -accel tcg,thread=multi \
        -cpu max \
        -smp 4 \
        -m 2G \
        -nodefaults \
        -display none \
        -no-reboot \
        -drive "if=pflash,format=raw,readonly=on,file=$share/edk2-aarch64-code.fd" \
        -drive "if=pflash,format=raw,file=$OUT_DIR/$name.vars.fd" \
        -serial "file:$OUT_DIR/$name.serial.log" \
        -netdev "tap,id=cable,ifname=$tap,script=no,downscript=no" \
        -device "virtio-net-pci,netdev=cable,mac=$mac,bootindex=1" \
        -drive "file=$disk,format=raw,if=none,id=disk" \
        -device virtio-blk-pci,drive=disk
    log "$name network-booting (arm64 uefi under emulation, mac $mac, serial log $(relative "$OUT_DIR/$name.serial.log"))"
}

serial_has() {
    tr -d '\r' <"$OUT_DIR/$1.serial.log" 2>/dev/null | grep -aqF -- "$2"
}

boot_until_registered() {
    local name=$1 mac=$2 tap=$3 arch=$4 timeout=$5 disk=$OUT_DIR/$1.raw
    rm -f "$disk" "$OUT_DIR/$name.serial.log"
    truncate -s 1G "$disk"
    if [[ $arch == arm64 ]]; then
        start_arm64_vm "$name" "$mac" "$disk" "$tap"
    else
        start_network_boot_vm "$name" "$mac" "$disk" "$tap"
    fi
    wait_until "$timeout" "the $name agent to register" host_registered "$mac"
    stop_process "$name"
    rm -f "$disk"
    local broken
    for broken in "Initramfs unpacking failed" "No space left on device" "Kernel panic" "Dropping to recovery shell"; do
        if serial_has "$name" "$broken"; then
            fail "$name: the serial log shows '$broken'"
        fi
    done
    log "$name: the agent registered and the serial log shows no initramfs unpacking error"
}

case_extra_initrds() {
    local name=$1 mac=$2 tap=$3 expected=$OUT_DIR/$1.expected
    start_case_server "$name" "[boot.profiles.x86_64]
arch = \"x86_64\"
initrds = [\"$FIXTURES/display-intel\", \"$FIXTURES/display-nvidia\", \"$FIXTURES/padding.cpio.gz\", \"$FIXTURES/init.cpio\"]"
    padded_concat "$expected" "$IMAGED_BOOT_ASSETS/x86_64/initramfs.cpio.xz" "$IMAGED_BOOT_ASSETS/x86_64/modules.cpio.xz" \
        "$FIXTURES/display-intel" "$FIXTURES/display-nvidia" "$FIXTURES/padding.cpio.gz" "$FIXTURES/init.cpio"
    assert_served "$name" x86_64 "$IMAGED_BOOT_ASSETS/x86_64/vmlinuz" "$expected"
    rm -f "$expected"
    VM_MEMORY=2G boot_until_registered "$name" "$mac" "$tap" x86_64 300
    serial_has "$name" "$INIT_MARKER" ||
        fail "$name: the /init from the uncompressed archive after the odd-length gzip archive did not run"
    log "$name: the /init from the last archive replaced the built-in one, after a $(stat -c %s "$FIXTURES/padding.cpio.gz") byte gzip archive"
    stop_process imaged-server
}

case_profile() {
    local name=$1 mac=$2 tap=$3 expected=$OUT_DIR/$1.expected version served=$OUT_DIR/$1.default
    version=$(cat "$FIXTURES/lts-version")
    start_case_server "$name" "[boot.profiles.lts]
arch = \"x86_64\"
kernel = \"$FIXTURES/lts/bzImage\"
initrds = [\"$FIXTURES/lts/modules.cpio.xz\"]

[boot.products]
\"$QEMU_PRODUCT\" = \"lts\""
    padded_concat "$expected" "$IMAGED_BOOT_ASSETS/x86_64/initramfs.cpio.xz" "$FIXTURES/lts/modules.cpio.xz"
    assert_served "$name" lts "$FIXTURES/lts/bzImage" "$expected"
    rm -f "$expected"
    curl -sSf --tftp-blksize 1468 -o "$served" "tftp://$SERVER_IP/x86_64/vmlinuz" || fail "$name: TFTP x86_64/vmlinuz failed"
    cmp -s "$served" "$IMAGED_BOOT_ASSETS/x86_64/vmlinuz" ||
        fail "$name: TFTP x86_64/vmlinuz, which reports no product, is not the built-in kernel"
    rm -f "$served"
    boot_until_registered "$name" "$mac" "$tap" x86_64 300
    serial_has "$name" "Linux version $version " || fail "$name: the serial log does not show linux $version booting"
    grep -aqF 'profile=lts' "$OUT_DIR/$name.server.log" ||
        fail "$name: the server log does not show profile lts selected for '$QEMU_PRODUCT'"
    log "$name: '$QEMU_PRODUCT' selected profile lts; linux $version booted with virtio drivers from the profile's modules archive"
    stop_process imaged-server
}

case_arm64() {
    local name=$1 mac=$2 tap=$3 expected=$OUT_DIR/$1.expected
    start_case_server "$name" "[boot.profiles.arm64]
arch = \"arm64\"
initrds = [\"$FIXTURES/display-nvidia-arm64\"]"
    padded_concat "$expected" "$IMAGED_BOOT_ASSETS/arm64/initramfs.cpio.xz" "$IMAGED_BOOT_ASSETS/arm64/modules.cpio.xz" \
        "$FIXTURES/display-nvidia-arm64"
    assert_served "$name" arm64 "$IMAGED_BOOT_ASSETS/arm64/vmlinuz" "$expected"
    rm -f "$expected"
    boot_until_registered "$name" "$mac" "$tap" arm64 1200
    stop_process imaged-server
}

run_case() {
    local index=$1 spec=$2
    local scenario=${spec%%:*}
    FIRMWARE=${spec##*:}
    local name=$scenario-$FIRMWARE mac
    mac=$(printf '52:54:00:e2:e2:%02x' "$index")
    case $scenario in
        extra-initrds) case_extra_initrds "$name" "$mac" "tap-$index" ;;
        profile) case_profile "$name" "$mac" "tap-$index" ;;
        arm64) case_arm64 "$name" "$mac" "tap-$index" ;;
        *) fail "unknown boot case '$scenario', expected extra-initrds, profile or arm64" ;;
    esac
}

if [[ -z ${E2E_IN_NAMESPACE:-} ]]; then
    require_commands nix cargo unshare ip qemu-system-x86_64 qemu-system-aarch64 dnsmasq curl jq cpio xz gzip cmp
    [[ -r /dev/kvm && -w /dev/kvm ]] || fail "/dev/kvm is not accessible"
    mkdir -p "$OUT_DIR"
    rm -rf "$OUT_DIR"/*-data "$OUT_DIR"/*.raw "$OUT_DIR"/*.log "$OUT_DIR"/*.fd "$OUT_DIR"/*.toml "$OUT_DIR/steps.txt"
    run_step fixtures build_fixtures
    run_step build-server build_server
fi
enter_cable_namespace "$0" "$@"
run_step network setup_cable_bridge
run_step dhcp start_dhcp_server
index=0
for spec in "${CASES[@]}"; do
    index=$((index + 1))
    run_step "$spec" run_case "$index" "$spec"
done
print_step_summary
log "PASS: boot customisations served over HTTP and TFTP and booted to a registered agent: ${CASES[*]}"

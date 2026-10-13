DISK_SIZE=128M
DISK_CONTENT=$OUT_DIR/disk-content
KNOWN_FILES=(
    esp/MARKER.TXT
    esp/EFI/BOOT/NOTE.TXT
    root/e2e/marker.txt
    root/e2e/random.bin
    root/usr/lib/os-release
    root/bin/busybox
)

partition_sectors() {
    sfdisk --json "$1" | jq -r --argjson n "$2" '.partitiontable.partitions[$n - 1] | "\(.start) \(.size)"'
}

write_partition() {
    local disk=$1 number=$2 image=$3 start size
    read -r start size < <(partition_sectors "$disk" "$number")
    (($(stat -c %s "$image") <= size * 512)) || fail "partition image $image does not fit partition $number"
    dd if="$image" of="$disk" bs=512 seek="$start" conv=notrunc,sparse status=none
}

extract_partition() {
    local disk=$1 number=$2 image=$3 start size
    read -r start size < <(partition_sectors "$disk" "$number")
    dd if="$disk" of="$image" bs=512 skip="$start" count="$size" status=none
}

partition_kib() {
    local start size
    read -r start size < <(partition_sectors "$1" "$2")
    echo $((size / 2))
}

write_root_content() {
    local root=$1
    mkdir -p "$root/e2e" "$root/etc" "$root/usr/lib" "$root/bin" "$root/proc" "$root/sys" "$root/dev" "$root/tmp"
    chmod 1777 "$root/tmp"
    cat >"$root/usr/lib/os-release" <<EOF
NAME="imaged e2e"
ID=imaged-e2e
PRETTY_NAME="imaged e2e test root"
EOF
    ln -s ../usr/lib/os-release "$root/etc/os-release"
    install -m 0755 "$E2E_BUSYBOX" "$root/bin/busybox"
    ln -s busybox "$root/bin/sh"
}

build_test_disk() {
    local disk=$1
    local work=$OUT_DIR/disk-build
    [[ -x ${E2E_BUSYBOX:-} ]] || fail "E2E_BUSYBOX must point at a static busybox binary (run inside: nix develop $REPO_DIR#e2e)"
    rm -rf "$DISK_CONTENT" "$work" "$disk"
    mkdir -p "$DISK_CONTENT/esp/EFI/BOOT" "$work"

    local marker
    marker="imaged e2e $(date -u +%FT%TZ) $(od -An -N8 -tx8 /dev/urandom | tr -d ' ')"
    echo "$marker" >"$DISK_CONTENT/esp/MARKER.TXT"
    echo "$marker" >"$DISK_CONTENT/esp/EFI/BOOT/NOTE.TXT"
    write_root_content "$DISK_CONTENT/root"
    echo "$marker" >"$DISK_CONTENT/root/e2e/marker.txt"
    head -c 4M /dev/urandom >"$DISK_CONTENT/root/e2e/random.bin"

    truncate -s "$DISK_SIZE" "$disk"
    sgdisk --clear \
        --new=1:1MiB:+32MiB --typecode=1:ef00 --change-name=1:esp \
        --new=2:0:0 --typecode=2:8300 --change-name=2:root \
        "$disk" >/dev/null

    mkfs.vfat -n ESP -C "$work/esp.img" "$(partition_kib "$disk" 1)" >/dev/null
    MTOOLS_SKIP_CHECK=1 mcopy -s -i "$work/esp.img" "$DISK_CONTENT/esp/"* ::/
    write_partition "$disk" 1 "$work/esp.img"

    mke2fs -q -t ext4 -L root -d "$DISK_CONTENT/root" "$work/root.img" "$(partition_kib "$disk" 2)k"
    write_partition "$disk" 2 "$work/root.img"

    rm -rf "$work"
    log "built $DISK_SIZE test disk $(relative "$disk"): GPT, p1 vfat ESP, p2 ext4 root with busybox and /etc/os-release; marker '$marker'"
}

build_blank_disk() {
    rm -f "$1"
    truncate -s "$DISK_SIZE" "$1"
}

read_known_files() {
    local disk=$1 dest=$2
    local work=$dest.partitions
    rm -rf "$dest" "$work"
    mkdir -p "$dest" "$work"

    extract_partition "$disk" 1 "$work/esp.img"
    fsck.fat -n "$work/esp.img" >"$work/esp.fsck" 2>&1 || fail "fsck.fat reports errors on ${disk##*/} p1: $(cat "$work/esp.fsck")"
    extract_partition "$disk" 2 "$work/root.img"
    e2fsck -fn "$work/root.img" >"$work/root.fsck" 2>&1 || fail "e2fsck reports errors on ${disk##*/} p2: $(cat "$work/root.fsck")"

    local file
    for file in "${KNOWN_FILES[@]}" root/etc/hostname; do
        mkdir -p "$(dirname "$dest/$file")"
        case $file in
            esp/*) MTOOLS_SKIP_CHECK=1 mcopy -n -i "$work/esp.img" "::/${file#esp/}" "$dest/$file" ;;
            root/*) debugfs -R "dump /${file#root/} $dest/$file" "$work/root.img" 2>/dev/null ;;
        esac
    done

    rm -rf "$work"
}

partition_layout() {
    sfdisk --json "$1" | jq -S '.partitiontable | del(.device) | .partitions |= map(del(.node))'
}

verify_deployed_disk() {
    local source=$1 deployed=$2 hostname=$3
    local actual
    actual=$OUT_DIR/verify/$(basename "$deployed" .raw)

    diff <(partition_layout "$source") <(partition_layout "$deployed") >&2 ||
        fail "partition table of ${deployed##*/} differs from ${source##*/}"
    log "partition table (disk GUID, partition GUIDs, offsets) matches the source disk"

    read_known_files "$deployed" "$actual"
    local file
    for file in "${KNOWN_FILES[@]}"; do
        cmp "$DISK_CONTENT/$file" "$actual/$file" >&2 || fail "$file on ${deployed##*/} differs from the source disk"
    done
    log "ESP and ext4 files match: $(cat "$actual/root/e2e/marker.txt")"
    log "random.bin sha256 $(sha256sum <"$actual/root/e2e/random.bin" | cut -c1-16)… identical"

    [[ -f $actual/root/etc/hostname ]] || fail "${deployed##*/} has no /etc/hostname; the post-install script did not run"
    local written
    written=$(cat "$actual/root/etc/hostname")
    [[ $written == "$hostname" ]] || fail "/etc/hostname on ${deployed##*/} is '$written', expected '$hostname'"
    log "/etc/hostname is '$written', written by the e2e-hostname script"
}

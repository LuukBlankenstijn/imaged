DISK_SIZE=128M
PARTITION_DISK_SIZE=192M
SWAP_UUID=5d1f7a3e-2b4c-4e8f-9a61-0c3b7d2e9f14
DISK_CONTENT=$OUT_DIR/disk-content
PARTITION_SNAPSHOTS=$OUT_DIR/before
BTRFS_CONTENT=$OUT_DIR/btrfs-content
BTRFS_FILES=(
    e2e/marker.txt
    e2e/random.bin
)
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

partition_sha256() {
    local start size
    read -r start size < <(partition_sectors "$1" "$2")
    dd if="$1" bs=1M iflag=skip_bytes,count_bytes skip=$((start * 512)) count=$((size * 512)) status=none |
        sha256sum | cut -d' ' -f1
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
        --new=2:0:-16MiB --typecode=2:8300 --change-name=2:root \
        --new=3:0:0 --typecode=3:8200 --change-name=3:swap \
        "$disk" >/dev/null

    mkfs.vfat -n ESP -C "$work/esp.img" "$(partition_kib "$disk" 1)" >/dev/null
    MTOOLS_SKIP_CHECK=1 mcopy -s -i "$work/esp.img" "$DISK_CONTENT/esp/"* ::/
    write_partition "$disk" 1 "$work/esp.img"

    mke2fs -q -t ext4 -L root -d "$DISK_CONTENT/root" "$work/root.img" "$(partition_kib "$disk" 2)k"
    write_partition "$disk" 2 "$work/root.img"

    truncate -s "$(partition_kib "$disk" 3)K" "$work/swap.img"
    chmod 0600 "$work/swap.img"
    mkswap -L e2e-swap -U "$SWAP_UUID" "$work/swap.img" >/dev/null
    write_partition "$disk" 3 "$work/swap.img"

    rm -rf "$work"
    log "built $DISK_SIZE test disk $(relative "$disk"): GPT, p1 vfat ESP, p2 ext4 root with busybox and /etc/os-release, p3 swap $SWAP_UUID; marker '$marker'"
}

build_blank_disk() {
    rm -f "$1"
    truncate -s "$DISK_SIZE" "$1"
}

build_partition_target_disk() {
    local disk=$1 root_size=${2:-96MiB} disk_size=${3:-$PARTITION_DISK_SIZE} name
    name=$(basename "$disk" .raw)
    local work=$OUT_DIR/disk-build
    rm -rf "$work" "$disk"
    mkdir -p "$work/esp/EFI/BOOT" "$work/root/e2e" "$work/data/e2e" "$PARTITION_SNAPSHOTS"

    truncate -s "$disk_size" "$disk"
    sgdisk --clear \
        --new=1:1MiB:+32MiB --typecode=1:ef00 --change-name=1:esp \
        --new=2:0:+"$root_size" --typecode=2:8300 --change-name=2:root \
        --new=3:0:0 --typecode=3:8300 --change-name=3:data \
        "$disk" >/dev/null

    echo "ESP of $name, never written by a partition deploy" >"$work/esp/EFI/BOOT/TARGET.TXT"
    mkfs.vfat -n ESP -C "$work/esp.img" "$(partition_kib "$disk" 1)" >/dev/null
    MTOOLS_SKIP_CHECK=1 mcopy -s -i "$work/esp.img" "$work/esp/"* ::/
    write_partition "$disk" 1 "$work/esp.img"

    echo "root filesystem of $name before the deploy" >"$work/root/e2e/stale.txt"
    mke2fs -q -t ext4 -L root -d "$work/root" "$work/root.img" "$(partition_kib "$disk" 2)k"
    write_partition "$disk" 2 "$work/root.img"

    head -c 2M /dev/urandom >"$work/data/e2e/random.bin"
    mke2fs -q -t ext4 -L data -d "$work/data" "$work/data.img" "$(partition_kib "$disk" 3)k"
    write_partition "$disk" 3 "$work/data.img"

    partition_layout "$disk" >"$PARTITION_SNAPSHOTS/$name.layout.json"
    partition_sha256 "$disk" 1 >"$PARTITION_SNAPSHOTS/$name.p1.sha256"
    partition_sha256 "$disk" 3 >"$PARTITION_SNAPSHOTS/$name.p3.sha256"
    rm -rf "$work"
    log "built $disk_size partition target disk $(relative "$disk"): GPT, p1 vfat ESP, p2 $root_size ext4 root holding /e2e/stale.txt, p3 ext4 data"
}

build_btrfs_filesystem() {
    local image=$1
    rm -rf "$BTRFS_CONTENT" "$image"
    mkdir -p "$BTRFS_CONTENT/e2e"
    local marker
    marker="imaged e2e btrfs $(date -u +%FT%TZ) $(od -An -N8 -tx8 /dev/urandom | tr -d ' ')"
    echo "$marker" >"$BTRFS_CONTENT/e2e/marker.txt"
    head -c 2M /dev/urandom >"$BTRFS_CONTENT/e2e/random.bin"
    truncate -s 256M "$image"
    mkfs.btrfs -q -L e2e-btrfs -m single -d single -r "$BTRFS_CONTENT" --shrink "$image" >/dev/null
    log "built btrfs filesystem $(relative "$image") rootless with --shrink: $(stat -c %s "$image") bytes, UUID $(blkid -p -o value -s UUID "$image"); marker '$marker'"
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
    local swap=$OUT_DIR/verify/$(basename "$deployed" .raw).swap
    extract_partition "$deployed" 3 "$swap"
    [[ $(blkid -p -o value -s UUID "$swap") == "$SWAP_UUID" && $(blkid -p -o value -s LABEL "$swap") == e2e-swap ]] ||
        fail "swap on ${deployed##*/} p3 is not $SWAP_UUID/e2e-swap: $(blkid -p "$swap")"
    rm -f "$swap"
    log "swap header restored with UUID $SWAP_UUID and label e2e-swap"
    log "random.bin sha256 $(sha256sum <"$actual/root/e2e/random.bin" | cut -c1-16)… identical"

    [[ -f $actual/root/etc/hostname ]] || fail "${deployed##*/} has no /etc/hostname; the post-install script did not run"
    local written
    written=$(cat "$actual/root/etc/hostname")
    [[ $written == "$hostname" ]] || fail "/etc/hostname on ${deployed##*/} is '$written', expected '$hostname'"
    log "/etc/hostname is '$written', written by the e2e-hostname script"
}

ext_filesystem_bytes() {
    local info count size
    info=$(dumpe2fs -h "$1" 2>/dev/null)
    count=$(sed -n 's/^Block count: *//p' <<<"$info")
    size=$(sed -n 's/^Block size: *//p' <<<"$info")
    echo $((count * size))
}

verify_other_partitions_unchanged() {
    local disk=$1 name
    name=$(basename "$disk" .raw)
    diff "$PARTITION_SNAPSHOTS/$name.layout.json" <(partition_layout "$disk") >&2 ||
        fail "the partition table of ${disk##*/} changed"
    log "partition table (disk GUID, partition GUIDs, offsets, names) unchanged"
    local number
    for number in 1 3; do
        [[ $(partition_sha256 "$disk" "$number") == "$(cat "$PARTITION_SNAPSHOTS/$name.p$number.sha256")" ]] ||
            fail "p$number of ${disk##*/} changed"
    done
    log "ESP (p1) and data (p3) byte-identical to before the deploy"
}

verify_partition_deploy() {
    local disk=$1 hostname=$2 fs_uuid=$3 image_bytes=$4 name
    name=$(basename "$disk" .raw)
    local actual=$OUT_DIR/verify/$name
    rm -rf "$actual"
    mkdir -p "$actual"

    verify_other_partitions_unchanged "$disk"

    local root=$actual/root.img
    extract_partition "$disk" 2 "$root"
    e2fsck -fn "$root" >"$actual/root.fsck" 2>&1 || fail "e2fsck reports errors on ${disk##*/} p2: $(cat "$actual/root.fsck")"
    local uuid
    uuid=$(blkid -p -o value -s UUID "$root")
    [[ $uuid == "$fs_uuid" ]] || fail "${disk##*/} p2 holds filesystem $uuid, expected the image's $fs_uuid"

    local file
    for file in "${KNOWN_FILES[@]}" root/etc/hostname; do
        [[ $file == root/* ]] || continue
        mkdir -p "$(dirname "$actual/$file")"
        debugfs -R "dump /${file#root/} $actual/$file" "$root" 2>/dev/null
        [[ $file == root/etc/hostname ]] && continue
        cmp "$DISK_CONTENT/$file" "$actual/$file" >&2 || fail "$file on ${disk##*/} p2 differs from the image"
    done
    if debugfs -R 'ls -p /e2e' "$root" 2>/dev/null | grep -q '/stale.txt/'; then
        fail "${disk##*/} p2 still holds /e2e/stale.txt from before the deploy"
    fi
    log "p2 holds filesystem $uuid with the image's files ($(cat "$actual/root/e2e/marker.txt")) and no longer /e2e/stale.txt"

    [[ -f $actual/root/etc/hostname ]] || fail "${disk##*/} p2 has no /etc/hostname; the post-install script did not run"
    local written
    written=$(cat "$actual/root/etc/hostname")
    [[ $written == "$hostname" ]] || fail "/etc/hostname on ${disk##*/} p2 is '$written', expected '$hostname'"
    log "/etc/hostname is '$written', written by the e2e-hostname script"

    local fs_bytes partition_bytes
    fs_bytes=$(ext_filesystem_bytes "$root")
    partition_bytes=$(stat -c %s "$root")
    ((image_bytes < partition_bytes)) || fail "the image's filesystem ($image_bytes bytes) already fills ${disk##*/} p2 ($partition_bytes bytes)"
    ((fs_bytes == partition_bytes)) ||
        fail "the filesystem on ${disk##*/} p2 holds $fs_bytes bytes, it did not grow to fill the $partition_bytes-byte partition"
    log "the filesystem grew from the image's $image_bytes bytes to fill the $partition_bytes-byte partition"
    rm -f "$root"
}

btrfs_filesystem_bytes() {
    btrfs inspect-internal dump-super "$1" | awk '$1 == "total_bytes" { print $2 }'
}

verify_btrfs_deploy() {
    local disk=$1 fs_uuid=$2 image_bytes=$3 name
    name=$(basename "$disk" .raw)
    local actual=$OUT_DIR/verify/$name
    rm -rf "$actual"
    mkdir -p "$actual/files"

    verify_other_partitions_unchanged "$disk"

    local fs=$actual/p2.img
    extract_partition "$disk" 2 "$fs"
    btrfs check --readonly "$fs" >"$actual/p2.check" 2>&1 ||
        fail "btrfs check reports errors on ${disk##*/} p2: $(cat "$actual/p2.check")"
    log "btrfs check --readonly is clean on p2"
    local uuid
    uuid=$(blkid -p -o value -s UUID "$fs")
    [[ $(blkid -p -o value -s TYPE "$fs") == btrfs && $uuid == "$fs_uuid" ]] ||
        fail "${disk##*/} p2 holds $(blkid -p "$fs"), expected the image's btrfs filesystem $fs_uuid"

    btrfs restore "$fs" "$actual/files" >"$actual/p2.restore" 2>&1 ||
        fail "btrfs restore cannot read ${disk##*/} p2: $(cat "$actual/p2.restore")"
    local file
    for file in "${BTRFS_FILES[@]}"; do
        cmp "$BTRFS_CONTENT/$file" "$actual/files/$file" >&2 || fail "$file on ${disk##*/} p2 differs from the image"
    done
    [[ ! -e $actual/files/e2e/stale.txt ]] || fail "${disk##*/} p2 still holds /e2e/stale.txt from before the deploy"
    log "p2 holds btrfs filesystem $uuid with the image's files ($(cat "$actual/files/e2e/marker.txt"), random.bin sha256 $(sha256sum <"$actual/files/e2e/random.bin" | cut -c1-16)…)"

    local fs_bytes partition_bytes
    fs_bytes=$(btrfs_filesystem_bytes "$fs")
    partition_bytes=$(stat -c %s "$fs")
    ((image_bytes < partition_bytes)) || fail "the image's filesystem ($image_bytes bytes) already fills ${disk##*/} p2 ($partition_bytes bytes)"
    ((fs_bytes == partition_bytes)) ||
        fail "the btrfs filesystem on ${disk##*/} p2 has total_bytes $fs_bytes, it did not grow to fill the $partition_bytes-byte partition"
    log "the btrfs filesystem grew from the image's $image_bytes bytes to total_bytes $fs_bytes, filling the partition"
    rm -f "$fs"
}

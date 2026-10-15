#!/bin/sh
/bin/busybox --install -s /bin
/bin/busybox mount -t proc none /proc
/bin/busybox mount -t sysfs none /sys
/bin/busybox mount -t devtmpfs none /dev
echo /bin/modprobe > /proc/sys/kernel/modprobe
/bin/mdev -d

load_drivers() {
    find /sys/devices -name modalias -exec cat {} + 2>/dev/null | sort -u | xargs -r modprobe -qa
}

load_drivers
load_drivers

if [ -d /sys/firmware/efi/efivars ]; then
    modprobe -q efivarfs
    mount -t efivarfs efivarfs /sys/firmware/efi/efivars
fi

for backlight in /sys/class/backlight/*; do
    [ -f "$backlight/max_brightness" ] || continue
    [ "$(cat "$backlight/brightness")" = "0" ] || continue
    cat "$backlight/max_brightness" > "$backlight/brightness"
done

i=0
while [ "$i" -lt 60 ]; do
    load_drivers
    linked=
    for iface in /sys/class/net/*; do
        [ "${iface##*/}" = "lo" ] && continue
        ip link set "${iface##*/}" up
        [ "$(cat "$iface/carrier" 2>/dev/null)" = "1" ] && linked=1
    done
    [ -n "$linked" ] && break
    i=$((i + 1))
    sleep 1
done

hwclock -s 2>/dev/null

lsblk -rno NAME,FSTYPE | while read -r name fstype; do
    case "$fstype" in
        ext2|ext3|ext4)
            echo "Checking /dev/$name"
            E2FSCK_CONFIG=/etc/e2fsck.conf e2fsck -f -p "/dev/$name"
            ;;
        vfat)
            echo "Checking /dev/$name"
            fsck.fat -a "/dev/$name"
            ;;
    esac
done

echo "Running dhcp to get an ip"
/bin/ipconfig -d all

for arg in $(cat /proc/cmdline); do
    case "$arg" in
        img_srv=*) SERVER_ADDR="${arg#img_srv=}" ;;
    esac
done

if [ -z "${SERVER_ADDR:-}" ]; then
    echo "ERROR: Kernel parameter 'img_srv' not found!"
    exec /bin/sh
fi

echo "Starting imaged-agent at $SERVER_ADDR"
if [ -c /dev/tty1 ]; then
    dmesg -n 3
    openvt -c 2 -- /bin/sh -c 'while :; do echo; echo "imaged shell, Alt+F1 returns to the dashboard"; /bin/sh; done'
    openvt -c 1 -s -w -- /bin/imaged-agent "$SERVER_ADDR"
else
    /bin/imaged-agent "$SERVER_ADDR"
fi

echo "imaged-agent exited. Dropping to recovery shell."
exec /bin/sh

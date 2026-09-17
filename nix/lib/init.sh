#!/bin/sh
/bin/busybox --install -s /bin
/bin/busybox mount -t proc none /proc
/bin/busybox mount -t sysfs none /sys
/bin/busybox mount -t devtmpfs none /dev

for backlight in /sys/class/backlight/*; do
    [ -f "$backlight/max_brightness" ] || continue
    [ "$(cat "$backlight/brightness")" = "0" ] || continue
    cat "$backlight/max_brightness" > "$backlight/brightness"
done

i=0
while [ "$i" -lt 10 ]; do
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

echo "Starting imaged-client at $SERVER_ADDR"
/bin/imaged-client "$SERVER_ADDR"

echo "imaged-client exited. Dropping to recovery shell."
exec /bin/sh

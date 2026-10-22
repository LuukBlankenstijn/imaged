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

echo "Running dhcp to get an ip"
/bin/ipconfig -d all

cat > /etc/udhcpc.script <<'EOF'
#!/bin/sh
case "$1" in
    bound|renew) ;;
    *) exit 0 ;;
esac
ip -4 addr show dev "$interface" | grep -q "inet $ip/" && exit 0
ip -4 addr flush dev "$interface"
ip addr add "$ip/$mask" dev "$interface"
for gateway in $router; do
    ip route add default via "$gateway" dev "$interface" && break
done
EOF
chmod +x /etc/udhcpc.script
for iface in $(ip -o -4 addr show | awk '$2 != "lo" { print $2 }' | sort -u); do
    address=$(ip -o -4 addr show dev "$iface" | awk '{ split($4, a, "/"); print a[1]; exit }')
    udhcpc -f -C -i "$iface" -r "$address" -s /etc/udhcpc.script >/dev/null 2>&1 &
done

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

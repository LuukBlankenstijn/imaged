#!/bin/sh
/bin/busybox --install -s /bin
/bin/busybox mount -t proc none /proc
/bin/busybox mount -t sysfs none /sys
/bin/busybox mount -t devtmpfs none /dev

echo "Running dhcp to get an ip"
/bin/ipconfig -d all
if [ -f /run/net-eth0.conf ]; then
    . /run/net-eth0.conf
    [ -n "$IPV4DNS0" ] && [ "$IPV4DNS0" != "0.0.0.0" ] && echo "nameserver $IPV4DNS0" > /etc/resolv.conf
    echo "$ROOTSERVER" > /run/imaging_server
fi

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

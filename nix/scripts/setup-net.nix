{ writeShellApplication, iproute2 }:

writeShellApplication {
  name = "setup-net";
  runtimeInputs = [ iproute2 ];
  text = ''
    bridge=br-netboot
    tap=tap0
    user=''${SUDO_USER:-$USER}

    if [ "$(id -u)" -ne 0 ]; then
      echo "must run as root: sudo setup-net" >&2
      exit 1
    fi

    if ! ip link show "$bridge" >/dev/null 2>&1; then
      echo "bridge $bridge not found; run 'docker compose up -d' first" >&2
      exit 1
    fi

    if ! ip link show "$tap" >/dev/null 2>&1; then
      ip tuntap add dev "$tap" mode tap user "$user"
      echo "created tap $tap owned by $user"
    fi

    current_master=$(ip -o link show "$tap" | sed -n 's/.* master \([^ ]*\).*/\1/p')
    if [ "$current_master" != "$bridge" ]; then
      ip link set "$tap" master "$bridge"
      echo "attached tap $tap to bridge $bridge"
    fi

    ip link set "$tap" up
  '';
}

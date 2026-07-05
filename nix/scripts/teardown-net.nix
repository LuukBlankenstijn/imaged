{ writeShellApplication, iproute2 }:

writeShellApplication {
  name = "teardown-net";
  runtimeInputs = [ iproute2 ];
  text = ''
    tap=tap0

    if [ "$(id -u)" -ne 0 ]; then
      echo "must run as root: sudo teardown-net" >&2
      exit 1
    fi

    if ip link show "$tap" >/dev/null 2>&1; then
      ip link set "$tap" down || true
      ip tuntap del dev "$tap" mode tap
      echo "removed tap $tap"
    fi
  '';
}

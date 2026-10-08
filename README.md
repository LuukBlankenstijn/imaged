# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then re-deploy it to other machines. Hosts boot over PXE into a small
client that talks to a central server. Inspired by the capture/deploy part of [The Fog Project](https://github.com/FOGProject/fogproject); multicast support is planned.

## Components

### `crates/core` — `imaged-core`

Rust library. Domain model, SQLite (sqlx) repositories for hosts, images and
tasks, the in-memory host connection registry, the multicast manager, image
storage, DI wiring, and the PXE routes. Image partitions are stored on disk
under `<data-dir>/images/`, next to the `<data-dir>/imaged.db` database.

### `crates/web` — `imaged-web`

Dioxus fullstack library that renders the dashboard (hydrated wasm client) and
mounts the server-side routes.

### `crates/api/ui` — `imaged-api-ui`

Dashboard server functions served under `/api/ui/*`.

### `crates/api/client` — `imaged-api-client`

Agent-facing server functions served under `/api/client/*`.

### `crates/server` — `imaged-server`

Binary and library package that wires `imaged-core`, `imaged-web` and the two
API crates together behind axum. By default it serves the dashboard and the
agent API from a single binary; `--web-bind-address` splits the dashboard UI/API
and the agent API onto two separate sockets. Styled with Tailwind CSS v4.

With the `server` feature it is also a library for embedding imaged in another
program: `Imaged::open(Config { data_dir, multicast_interface })` once per
process, then `imaged.serve(Listeners { machine, public_url, tftp, dashboard })`
returns a `Server`. `stop()` disconnects every agent, gives open requests a
short grace period and then cuts off whatever connection is left, after which
`serve` can be called again with new addresses; one `Server` runs at a time. The
machine listener only serves `/boot/*` and `/api/client/*`; the dashboard is
`Dashboard::Off`, `OnMachineListener` or `Separate(address)`.

Run (dev):

```sh
dx serve --package imaged-server
```

Bundle (production):

```sh
dx bundle --release --platform web --package imaged-server
```

Listens on `--bind-address` (the NixOS module defaults to `0.0.0.0:8080`); an
optional `--web-bind-address` serves the dashboard on a separate socket. The
database and images live in `--data-dir` (default: the working directory). iPXE
scripts point machines at `--public-base` (`PUBLIC_BASE`, default
`localhost:8080`), over HTTPS when `--use-https` is passed or `USE_HTTPS` is
truthy (anything but empty, `0`, `false`, `no` or `off`).

### `crates/client` — `imaged-client`

Rust binary that runs on a PXE-booted machine. On start it sends its state to
the server (mac address, disk size) and processes capture / deploy tasks issued
back over SSE. Uses `partclone` for filesystem-aware imaging.

```sh
cargo run -p imaged-client -- http://<server>:8080
```

## Dev environment

A `flake.nix` provides the toolchain: Rust with the `wasm32-unknown-unknown`
target, `dx` (dioxus-cli), Tailwind CSS v4, wasm-bindgen, binaryen and sqlx-cli.
With direnv:

```sh
direnv allow
```

It also provides everything else the project needs: partclone, udp-cast, the
initramfs packages and scripts, and the test-VM scripts. The x86_64 and arm64
kernels are nixpkgs' `linuxPackages_latest`; the initramfs carries its modules
(minus GPU, media, sound and wireless) and loads them by modalias at boot.
`build-initramfs` builds the initramfs for every arch, or only the ones passed
as arguments (`build-initramfs arm64`).

## Network boot

DHCP hands bare PXE ROMs an iPXE binary over TFTP by client architecture
(option 93): `undionly.kpxe` for BIOS (0), `ipxe.efi` for x86 UEFI (6, 7, 9)
and `ipxe-arm64.efi` for arm64 UEFI (11). iPXE then chains
`/boot/boot.ipxe`, which reports its build architecture so the server answers
with the matching kernel and initramfs. See `dnsmasq.conf` for a working setup.

`imaged-server --tftp-bind-address 0.0.0.0:69` also serves the iPXE binaries,
kernels and initramfs over TFTP, so no separate TFTP server is needed.

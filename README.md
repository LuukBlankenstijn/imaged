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

Images can be exported and imported as `.imaged` archives: an uncompressed tar
holding `manifest.json` (name, capture date, partitions and a sha256 per file),
`parttable.bin` and one `p-<n>.pcl` per partition
(`GET /api/ui/images/{id}/export`, `POST /api/ui/images/import`). An import
stays `importing` until every checksum matches and then turns `ready`; a
rejected import turns `faulted`, and one cut short by a crash is marked faulted
the next time the server opens its data dir.

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
program: `Imaged::open(Config { data_dir, multicast_interface, script_proxy })` once per
process, then `imaged.serve(Listeners { machine, public_url, tftp, dashboard })`
returns a `Server`. `stop()` disconnects every agent, gives open requests a
short grace period and then cuts off whatever connection is left, after which
`serve` can be called again with new addresses; one `Server` runs at a time. The
machine listener only serves `/boot/*` and `/api/client/*`; the dashboard is
`Dashboard::Off`, `OnMachineListener` or `Separate(address)`.

The server side (core, both API crates and the `server` library) also builds
for Windows (`x86_64-pc-windows-gnu`). Multicast is Linux-only: on other
platforms a multicast task fails with "multicast is only supported on Linux".

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
truthy (anything but empty, `0`, `f`, `false`, `n`, `no` or `off`).

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

## Post-install scripts

Scripts are a library on the server (`/api/ui/scripts`): a unique name (letters,
digits, `-`, `_`, `.`), a body and a list of declared inputs. An input has a
name (`[a-z0-9_]+`), a label, a kind (`text`, `number` or `choice` with its
choices), whether it is required, an optional default and whether its value is
given per machine or shared by every machine of a deployment. An image keeps an
ordered list of scripts (`POST /api/ui/images/scripts`) and always runs their
current version; a script that an image still uses cannot be deleted. Images
also carry a root partition (`POST /api/ui/images/root-partition`), the
partition the scripts run in. A capture finds it by mounting every ext
partition read-only after cloning it: when exactly one holds `/etc/os-release`
it becomes the image's root partition, otherwise it is left unset for the
dashboard to choose. Archives carry it as the optional `root_partition`
manifest field.

After a deploy or multicast has restored every partition, the client fetches
the image's scripts (`GET /api/client/tasks/{id}/scripts`) and, if there are
any, mounts the root partition under `/tmp/imaged-root`, then every entry of
the image's `/etc/fstab` that names a partition of the restored disk (by
`UUID=`, `LABEL=`, `PARTUUID=`, `PARTLABEL=` or device path), then `/proc`,
`/sys` and `/dev`. Each script is written to `/tmp` inside that root and run
with `chroot`, through its shebang or `/bin/sh` without one, with these
variables:

- `IMAGED_INPUT_<NAME>` for each of its inputs that has a value (the name
  uppercased),
- `IMAGED_MAC`, `IMAGED_IP` and `IMAGED_SERVER` of the client,
- `http_proxy`, `https_proxy`, `HTTP_PROXY` and `HTTPS_PROXY` when the server
  runs with `--script-proxy <url>` (`SCRIPT_PROXY`, `Config::script_proxy`).

Stdout and stderr go to the client log and, as the script runs, to the server,
which keeps the full output and exit code of every run per task host
(`GET /api/ui/tasks/{task_id}/hosts/{host_id}/scripts`). A script that exits
non-zero stops the deploy and fails that host with the script's name; the
mounts are always undone in reverse order before the client reports back and
reboots.

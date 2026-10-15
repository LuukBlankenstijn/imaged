# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then re-deploy it to other machines. Hosts boot over PXE into a small
agent that talks to a central server. Inspired by the capture/deploy part of [The Fog Project](https://github.com/FOGProject/fogproject); multicast support is planned.

## Components

### `crates/model` — `imaged-model`

Types and rules shared by the server, the agent, the dashboard and the packer:
`AppError` with optional axum (`axum-error`), sqlx (`sqlx-error`) and dioxus
server function (`serverfn`) integrations, the agent protocol
(`imaged_model::agent`: route templates, path builders, request and response
types, headers; the websocket events are `ServerEvent`), the multicast group
and port math, the script input and script name rules, and the rules for the
variables an embedding program hands to scripts. Builds for wasm; the
`logging` feature adds the tracing setup the binaries share.

### `crates/archive` — `imaged-archive`

The `.imaged` archive format: an uncompressed tar holding `manifest.json` first
(format version, name, capture date, partitions, the optional root partition
and post-install scripts with their inputs, and a sha256 per file), then
`parttable.bin` (an `sgdisk --backup` of the GPT), one `p-<n>.pcl` per
partition in manifest order (partclone output compressed with zstd) and one
`scripts/<name>.sh` per script. The manifest, the partition table and every
script are limited to 1 MiB. The `manifest` module is IO-free and validates
file names, partition numbers, the root partition and the scripts; an async
writer refuses an invalid manifest before writing a byte, and an async reader
checks the manifest first and every checksum while it unpacks. The server's
image storage uses the same file names.

### `crates/disk` — `imaged-disk`

Disk imaging shared by the agent and `imaged-pack`, so both encode partitions
identically: partition table backup and restore with `sgdisk`, `partclone`
clones piped through zstd and restores piped back through it, and the root
partition probe. The probe runs `debugfs -R 'stat /etc/os-release'` against
each ext partition without mounting it, so it needs no root; a symlinked
`os-release` counts. `partclone` always gets `--logfile`: without it partclone
writes `/var/log/partclone.log` and, when not root, outputs nothing.

### `crates/core` — `imaged-core`

Rust library. Domain model, SQLite (sqlx) repositories for hosts, images and
tasks, the in-memory host connection registry, the multicast manager, image
storage and DI wiring. Image partitions are stored on disk
under `<data-dir>/images/`, next to the `<data-dir>/imaged.db` database.

Images can be exported and imported as `imaged-archive` archives
(`GET /api/ui/images/{id}/export`, `POST /api/ui/images/import`). An
import stays `importing` until every checksum matches and the upload has been
read to its end, and then turns `ready`; a
rejected import turns `faulted`, and one cut short by a crash is marked faulted
the next time the server opens its data dir. Imported scripts replace the body
and inputs of the library script with the same name (or are added) and are
attached to the new image in their archived order; archives without scripts or
a root partition still import.

### `crates/boot` — `imaged-boot`

Network boot: the PXE routes (`imaged_boot::pxe::router(public_url)`:
`/boot/boot.ipxe`, `/boot/manifest.ipxe`, `/boot/{arch}/vmlinuz` and
`/boot/{arch}/initramfs.cpio.xz`) and the read-only TFTP server
(`imaged_boot::tftp::bind(address)`) that hands out the iPXE binaries, kernels
and initramfs. All of them are embedded at compile time, so the server ships as
one executable. The build script reads them from `IMAGED_BOOT_ASSETS` (an
absolute path) when set, otherwise from `assets/` at the repository root, and
fails naming every missing file. The directory holds `ipxe.efi`,
`ipxe-arm64.efi`, `undionly.kpxe` and `x86_64/` and `arm64/` with `vmlinuz`
and `initramfs.cpio.xz` each; `nix build .#boot-assets` builds exactly that
directory. Nothing else depends on the assets: `imaged-core` and the API
crates build and test without them.

### `crates/web` — `imaged-web`

Dioxus fullstack library that renders the dashboard (hydrated wasm client) and
mounts the server-side routes. The Scripts page edits the script library; each image
lists its scripts and root partition; the deploy and multicast dialogs ask for
the inputs of the image's scripts (shared values once, per-machine values per
host); and the Tasks page shows each host's script output.

### `crates/api/ui` — `imaged-api-ui`

Dashboard server functions served under `/api/ui/*`.

### `crates/api/agent` — `imaged-api-agent`

Plain axum routes served under `/api/client/*` for the agent. The protocol
both sides speak (route templates, path builders, request and response types,
headers and the JSON events sent over the websocket) lives in
`imaged_model::agent`.

### `crates/server` — `imaged-server`

Binary and library package that wires `imaged-core`, `imaged-boot`,
`imaged-web` and the two API crates together behind axum. By default it serves the dashboard and the
agent API from a single binary; `--web-bind-address` splits the dashboard UI/API
and the agent API onto two separate sockets. Styled with Tailwind CSS v4.

With the `server` feature it is also a library for embedding imaged in another
program: `Imaged::open(Config { data_dir, multicast_interface, script_proxy, script_variables })` once per
process, then `imaged.serve(Listeners { machine, public_url, tftp, dashboard })`
returns a `Server`. `stop()` disconnects every agent, gives open requests a
short grace period and then cuts off whatever connection is left, after which
`serve` can be called again with new addresses; one `Server` runs at a time. The
machine listener only serves `/boot/*` and `/api/client/*`; the dashboard is
`Dashboard::Off`, `OnMachineListener` or `Separate(address)`. The embedding
program reads and feeds imaged through the same handle:
`imaged.import_archive(reader)` imports an image archive and returns the image,
`imaged.script_inputs(image_id)` lists the inputs declared by the image's
scripts in script order and `imaged.connected_hosts()` lists the ids of the
machines whose agent is connected.

The server side (core, boot, both API crates and the `server` library) also builds
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

### `crates/agent` — `imaged-agent`

Rust binary that runs on a PXE-booted machine. On start it connects to
`/api/client/stream` over a websocket with its state (mac address, disk size),
receives capture / deploy tasks over it as JSON events, and uploads and
downloads partition tables, partitions and script output over HTTP. Uses
`imaged-disk` (`partclone` for filesystem-aware imaging, `sgdisk` for the
partition table, `debugfs` for the root partition).

```sh
cargo run -p imaged-agent -- http://<server>:8080
```

### `crates/pack` — `imaged-pack`

Rust binary that turns a raw GPT disk image file into a `.imaged` archive
without root, KVM or a PXE boot. The archive matches what a capture produces,
so it imports through `POST /api/ui/images/import` and deploys like a captured
image.

```
imaged-pack <DISK> --name <NAME> [--root-partition <N>] [--script <PATH>]... (-o|--output) <OUT>
```

It reads the layout with `sfdisk --json` (only GPT disks; anything else fails
with `only GPT disks are supported, found <label>`) and backs up the partition
table with `sgdisk` into `parttable.bin`. Each partition is copied into a sparse
temp file, its filesystem read with `blkid -p`, and cloned with `imaged-disk`
into `p-<n>.pcl`; its size is its sector count times the sector size, as
during a capture. Unformatted partitions are skipped like a capture skips
them; ext2/3/4 and vfat are packed; any other filesystem fails the run.
`--root-partition <N>` sets the root partition (it must be one of the packed
partitions); without it the root partition is found by the same rule as a
capture and left unset when no single partition qualifies. The capture date is
the time of packing.

`--script <PATH>` attaches a post-install script and can be repeated. The file
is `<name>.sh`, the script name its stem; its inputs come from a sibling
`<name>.inputs.json` when there is one, otherwise it has none:

```json
[
  {
    "name": "hostname",
    "label": "Hostname",
    "description": "Name the machine announces on the network",
    "kind": "text",
    "required": true,
    "per_machine": true
  }
]
```

Scripts follow the server's library rules and names must be unique; they are
archived and attached in command-line order. The archive is written to
`<OUT>.partial` and renamed to `<OUT>` once complete; on failure nothing is
left behind. Temp files go to `TMPDIR`.

```sh
nix run github:LuukBlankenstijn/imaged#imaged-pack -- disk.img --name my-image --script setup.sh -o my-image.imaged
```

The nix package brings its runtime tools (`sgdisk`, `sfdisk`, `blkid`,
`debugfs`, `partclone`).

### `crates/e2e` — `imaged-e2e`

Rust HTTP, WebSocket and SSE tests against a real server.

## Dev environment

A `flake.nix` provides the toolchain: Rust with the `wasm32-unknown-unknown`
target, `dx` (dioxus-cli), Tailwind CSS v4, wasm-bindgen, binaryen and sqlx-cli.
With direnv:

```sh
direnv allow
```

It also provides everything else the project needs: partclone, udp-cast,
sgdisk, sfdisk and blkid (util-linux), e2fsprogs, dosfstools, mtools, the
initramfs packages and scripts, and the test-VM scripts; the imaging tests in
`imaged-disk` and `imaged-pack` run these tools. The x86_64 and arm64
kernels are nixpkgs' `linuxPackages_latest`; the initramfs carries its modules
(minus GPU, media, sound and wireless) and loads them by modalias at boot.
`build-initramfs` builds the initramfs for every arch, or only the ones passed
as arguments (`build-initramfs arm64`).

`imaged-boot` embeds the boot assets from `assets/` (see above). Entering the
dev shell copies the kernels and iPXE binaries there and `build-initramfs`
writes the initramfs there from the working tree, so on a fresh checkout:

```sh
nix develop
build-initramfs x86_64 arm64
cargo build
```

Tests run in one pass:

```sh
cargo test --workspace
```

CI runs the same suite as `nix build .#checks.x86_64-linux.tests`.

## Network boot

DHCP hands bare PXE ROMs an iPXE binary over TFTP by client architecture
(option 93): `undionly.kpxe` for BIOS (0), `ipxe.efi` for x86 UEFI (6, 7, 9)
and `ipxe-arm64.efi` for arm64 UEFI (11). iPXE then chains
`/boot/boot.ipxe`, which reports its build architecture so the server answers
with the matching kernel and initramfs. See `dev/dnsmasq.conf` (run with
`docker compose -f dev/compose.yaml up`) for a working setup.

`imaged-server --tftp-bind-address 0.0.0.0:69` also serves the iPXE binaries,
kernels and initramfs over TFTP, so no separate TFTP server is needed.

## Post-install scripts

Scripts are a library on the server (`/api/ui/scripts`): a unique name (letters,
digits, `-`, `_`, `.`), a body and a list of declared inputs. An input has a
name (`[a-z0-9_]+`), a label, an optional description (at most 1000
characters, shown under the field wherever its value is entered: the
dashboard's deploy and multicast dialogs and the provisioner), a kind (`text`,
`number` or `choice` with its choices), whether it is required, an optional
default and whether its value is given per machine or shared by every machine
of a deployment. An image keeps an
ordered list of scripts (`POST /api/ui/images/scripts`) and always runs their
current version; a script that an image still uses cannot be deleted. Images
also carry a root partition (`POST /api/ui/images/root-partition`), the
partition the scripts run in. A capture finds it after cloning every partition
by reading each ext partition with `debugfs`, without mounting it: when exactly
one holds `/etc/os-release` (a symlink counts) it becomes the image's root
partition, otherwise it is left unset for the dashboard to choose.
`imaged-pack` applies the same rule.

A deploy (`POST /api/ui/hosts/deploy`) or multicast
(`POST /api/ui/groups/multicast`) of an image with scripts takes the input
values as `inputs: { shared: { name: value }, per_machine: { host_id: { name:
value } } }`: shared inputs come from `shared`, per-machine inputs from each
host's entry. Before any task is created the values are checked (unknown or
misplaced names, required values, numbers, choices) and an empty value falls
back to the input's default; a rejected request is a 400, and an image with
scripts but no root partition is refused with a 412. The resolved values are
stored with each task host, so a retried or reconnecting host gets the same
ones.

After a deploy or multicast has restored every partition, the agent fetches
the image's scripts (`GET /api/client/tasks/{id}/scripts`) and, if there are
any, mounts the root partition under `/tmp/imaged-root`, then every entry of
the image's `/etc/fstab` that names a partition of the restored disk (by
`UUID=`, `LABEL=`, `PARTUUID=`, `PARTLABEL=` or device path), then `/proc`,
`/sys` and `/dev`. Symlinks on the way to `/etc/fstab` resolve inside the
image, so an absolute link such as NixOS's `/etc/fstab -> /etc/static/fstab`
finds the image's file. On UEFI machines the initramfs mounts `efivarfs`, and
the `/sys` bind mount carries it into the chroot, so a script can write boot
entries to NVRAM (`bootctl install`, `grub-install`, `efibootmgr`). Each
script is written to `/tmp` inside that root and run with `chroot`, through
its shebang or `/bin/sh` without one, with these variables:

- `IMAGED_INPUT_<NAME>` for each of its inputs that has a value (the name
  uppercased),
- `IMAGED_MAC`, `IMAGED_IP` and `IMAGED_SERVER` of the agent,
- `http_proxy`, `https_proxy`, `HTTP_PROXY` and `HTTPS_PROXY` when the server
  runs with `--script-proxy <url>` (`SCRIPT_PROXY`, `Config::script_proxy`).
- every entry of `Config::script_variables`, which an embedding program uses to
  hand scripts values such as credentials; names must be shell identifiers and
  cannot be one of the variables above.

Stdout and stderr go to the agent log and, as the script runs, to the server,
which keeps the full output and exit code of every run per task host
(`GET /api/ui/tasks/{task_id}/hosts/{host_id}/scripts`). A script that exits
non-zero stops the deploy and fails that host with the script's name; the
mounts are always undone in reverse order before the agent reports back and
reboots.

## End-to-end VM test

```sh
nix develop .#e2e -c e2e/run.sh
```

Runs as your user inside user, network and pid namespaces; it needs `/dev/kvm`
and `/dev/net/tun`, and real boot assets in `assets/` (or `IMAGED_BOOT_ASSETS`)
with an initramfs built from this tree (`build-initramfs x86_64`). The script:

1. builds the binaries it runs,
2. creates the test disks,
3. packs one with `imaged-pack` and a script that has a per-machine input,
4. imports the archive and checks that it is `ready` with root partition 2 and
   the script attached,
5. captures the same disk in a VM and checks that the agent found root
   partition 2,
6. exports the captured image and compares its structure with the packed
   archive,
7. deploys both images with a hostname input and checks each deployed disk:
   the same GPT, a clean fsck, the same files, and `/etc/hostname` set by the
   script.

`E2E_FIRMWARE` picks `bios` or `uefi`, `E2E_OUT` the directory for logs, disks
and archives (default `e2e/out`); `CARGO_TARGET_DIR` and `IMAGED_BOOT_ASSETS`
are honoured, and the assets the script checks are the ones it compiles in.

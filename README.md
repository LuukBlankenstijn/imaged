# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then deploy it to other machines, one at a time or to a group at once
over multicast. Hosts boot over PXE into a small agent that talks to a central
server. Inspired by the capture/deploy part of [The Fog Project](https://github.com/FOGProject/fogproject).

imaged has no authentication; see [Security](#security) before running it.

## Components

### `crates/model` — `imaged-model`

Types and rules shared by the server, the agent, the dashboard and the packer:
`AppError` with optional axum (`axum-error`), sqlx (`sqlx-error`) and dioxus
server function (`serverfn`) integrations, the agent protocol
(`imaged_model::agent`: route templates, path builders, request and response
types, headers, MAC address parsing; the websocket events are `ServerEvent`
down and `AgentEvent` up), the progress an agent reports
(`imaged_model::progress`), the multicast group and port math, the script input
and script name rules, and the rules for the variables an embedding program
hands to scripts. Builds for wasm; the `logging` feature adds the tracing setup
the binaries share.

### `crates/archive` — `imaged-archive`

The `.imaged` archive format: an uncompressed tar holding `manifest.json` first
(format version, name, capture date, sector size, partitions, the optional root
partition and post-install scripts with their inputs, and a sha256 per file),
then `parttable.bin` (an `sgdisk --backup` of the GPT), one `p-<n>.pcl` per
partition in manifest order and one `scripts/<name>.sh` per script. A `p-<n>.pcl`
is zstd-compressed partclone output, except for swap, where it is the
zstd-compressed swap header page. The manifest, the partition table and every
script are limited to 1 MiB.

The current format is 2; any other `format` is refused (`unsupported archive
format 1, expected 2`). `sector_size` is the logical sector size the LBAs in
`parttable.bin` refer to, a power of two from 512 to 65536. `partitions` must
not be empty and each `fstype` is one of `ext2`, `ext3`, `ext4`, `vfat` and
`swap`. Unknown manifest fields are ignored; a new payload encoding or a field
a reader must honour needs a format bump.

The `manifest` module is IO-free and validates the sector size, file names,
partition numbers, file systems, the root partition and the scripts; an async
writer refuses an invalid manifest before writing a byte, and an async reader
checks the manifest first and every checksum while it unpacks. The server's
image storage uses the same file names.

### `crates/disk` — `imaged-disk`

Disk imaging shared by the agent and `imaged-pack`, so both encode partitions
identically: partition table backup and restore with `sgdisk`, partition
clones piped through zstd and restores piped back through it, and the root
partition probe.

Only GPT disks are imaged: a backup of anything else fails with `only GPT disks
are supported, found <dos|no partition table|a protective MBR without a GPT
header>` before `sgdisk` runs. The backup reports the disk's logical sector size
and its partitions in bytes. Before a restore wipes the target it checks that
the target has the image's sector size (`the image's partition table is for
N-byte sectors, /dev/x has M-byte sectors`), that it is large enough for the
image's partitions (`/dev/x holds N bytes, the image's partitions need at least
M`; a larger disk is fine) and that the backup is intact; a refused target is
left untouched. After loading the table it runs `partprobe <disk>`, and a
failed re-read (a busy disk, for example) fails the restore.

ext2/3/4 and vfat partitions are cloned with `partclone`;
swap partitions are stored as their header page only (the `SWAPSPACE2`
signature with the UUID, label and size, 4 to 64 KiB) and restored by writing
that page back, so the restored swap keeps its UUID and label. A swap
partition holding a hibernation image is reported as `swsuspend` and refused.
A clone stream ends in an error when partclone fails, so a reader that reaches
its end without an error has the whole partition. The probe runs
`debugfs -R 'stat /etc/os-release'` against each ext partition without
mounting it, so it needs no root; a symlinked `os-release` counts. `partclone`
always gets `--logfile`: without it partclone writes
`/var/log/partclone.log` and, when not root, outputs nothing. Its stderr is
parsed: every data status line (fraction done, rate, time remaining) goes to the
caller's progress callback, the bitmap scan before it is skipped and any other
line is logged. A clone stream only ends once its last progress is reported.
The output of `sgdisk` and `partprobe` is logged too, and a failure carries it
in its error; nothing is printed to the caller's terminal.

### `crates/core` — `imaged-core`

Rust library. Domain model, SQLite (sqlx) repositories for hosts, images and
tasks, the in-memory host connection registry, the multicast manager, the
in-memory progress board, image storage and DI wiring. The progress board keeps
the latest progress each agent reported, until the host finishes, fails, is
cancelled or disconnects; a running multicast session adds its scuttlecast sender
stats (phase, rate, per-receiver lag and loss, the host holding it up) to the
same board. Image partitions are stored on disk
under `<data-dir>/images/`, next to the `<data-dir>/imaged.db` database (WAL
mode). Captured and imported files are fsynced before an image turns `ready`.
Hosts are keyed by their MAC address in lowercase colon form; each image records
the sector size of its partition table.

Images can be exported and imported as `imaged-archive` archives
(`GET /api/ui/images/{id}/export`, only for a `ready` image, and
`POST /api/ui/images/import`, which takes the raw archive and returns the image
once it is processed). An archive whose manifest is invalid is refused before
an image exists. A valid one stays `importing` until every checksum matches and
the upload has been read to its end, and then turns `ready`; an import rejected
after that turns `faulted`, and one cut short by a crash is marked faulted the
next time the server opens its data dir. Each imported script is matched to the
library by name: an identical script is reused, a different one that no other
image uses is replaced, and otherwise the import adds it as `<name>-2`, `-3`, …
(reusing an identical copy under such a name), so it never changes the scripts
of another image. Scripts are attached to the new image in their archived
order; archives without scripts or a root partition still import.

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
mounts the server-side routes. The Scripts page edits the script library
(choices one per line); each image lists its scripts and root partition; the
deploy and multicast dialogs need an explicit image, list every machine whose
disk will be erased and ask for the inputs of the image's scripts (shared
values once, per-machine values per host). Hosts without a usable disk show
why. The Tasks page filters by status, type and host and pages through the
history 50 tasks at a time; it shows each host's name, MAC and script output,
and a retry of a deploy or multicast asks for confirmation first. Active tasks
get a progress bar averaging their hosts, each running host its own bar with the
partition being imaged, its rate and eta, and a multicast also the sender stats
and per-host receiver stats used to decide whom to kick. Progress is streamed
from `/api/ui/task-progress` at most once a second. A failed refresh keeps the
last loaded list on screen with an error banner.

### `crates/api/ui` — `imaged-api-ui`

Dashboard server functions served under `/api/ui/*`, without authentication.
They are the dashboard's own RPC: a JSON body is keyed by the Rust parameter
names, so a function taking `req: DeployRequest` takes `{"req": {...}}` and one
taking `id: i64` takes `{"id": 1}`. Every method other than GET, HEAD and
OPTIONS passes a cross-site check (see [Security](#security)).

- `GET /api/ui/tasks` is paged, newest first. Its query takes
  `status=active|done|failed|cancelled` (`active` is pending or running,
  `failed` includes partial), `type=capture|deploy|multicast|reboot`,
  `host_id=`, `before=` and `limit=` (default 50, at most 200), and it returns
  `{"tasks": [...], "next_cursor": n|null}`; pass `next_cursor` as `before` for
  the next page. Unknown query keys are refused. Every task host carries
  `host_name` and `mac_address`.
- `POST /api/ui/images/create` (`{"req": {"name", "host_id"}}`) starts a
  capture and returns `{"image": ..., "task": ...}`; `POST /api/ui/hosts/deploy`,
  `POST /api/ui/groups/multicast` and `POST /api/ui/hosts/reboot` return the
  created task.
- Capture, deploy and multicast refuse an unknown host (404) and a host
  without a usable disk (412) before creating anything; deploy and multicast
  also refuse an image that is not `ready` (412).
- `POST /api/ui/images/delete` is refused (412) while a pending or running
  deploy or multicast uses the image; a running capture of it is cancelled.
- Host JSON has `disk_size_bytes` or, for a host without a usable disk,
  `disk_error`.

### `crates/api/agent` — `imaged-api-agent`

Plain axum routes served under `/api/client/*` for the agent. The protocol
both sides speak (route templates, path builders, request and response types,
headers and the JSON events sent over the websocket) lives in
`imaged_model::agent`. An agent is identified by its `X-Agent-Mac` header;
colon- or dash-separated MACs in any case are accepted and stored as lowercase
colon form, anything else is a 400. The stream query carries exactly one of
`disk_size_bytes` and `disk_error`. The server sends one event,
`{"Current": task|null}`: on connect, whenever the host's current task changes
(a cancel or kick shows up as another task or `null`) and in reply to the
agent's `"Idle"`. Reporting a task finished or failed again with the same
outcome succeeds, so the agent can retry its report.

### `crates/server` — `imaged-server`

Binary and library package that wires `imaged-core`, `imaged-boot`,
`imaged-web` and the two API crates together behind axum. The machine listener
only serves `/boot/*` and `/api/client/*`; the dashboard and `/api/ui/*` get a
listener of their own (`listen.dashboard`, by default `127.0.0.1:8081`), can
share the machine listener (`"machine"`) or be turned off (`"off"`). Styled
with Tailwind CSS v4.

`listen.proxy = "192.168.100.1:3128"` also runs an HTTP proxy (the
`imaged-proxy` crate) so post-install scripts can reach the internet from a
network without a router. It tunnels `CONNECT` to ports 443 and 22 (SSH via
`ProxyCommand`), forwards plain HTTP to port 80 only and refuses private and
reserved targets (also when embedded in an IPv6 address: NAT64, 6to4,
IPv4-compatible or mapped) and every address of the server's own interfaces.
Without `script_proxy`, scripts get it as `http_proxy`: the bind address, or
the `public_url` host when bound to an unspecified address. Bind it to the
netboot interface: anyone who reaches the port can use it.

With the `server` feature it is also a library for embedding imaged in another
program. `Imaged::open(config)` once per process, with
`Config::new(data_dir, multicast_interface)` and its `script_proxy` and
`script_variables` fields set as needed; then `imaged.serve(listeners)` with
`Listeners::new(machine, public_url)` and its `tftp`, `proxy` and `dashboard`
fields (`Dashboard::Off` unless set to `OnMachineListener` or
`Separate(address)`) returns a `Server`. `Config`, `Listeners` and `Dashboard`
are `#[non_exhaustive]`. `serve` refuses a `public_url` that is not an
`http://` or `https://` URL with a host (`InvalidInput`) before binding
anything. A dashboard needs the bundle's `public/` directory next to the
executable or at `DIOXUS_PUBLIC_PATH`. `stop()` disconnects every agent, gives
open requests a 3 second grace period and then cuts off whatever connection is
left, after which `serve` can be called again with new addresses; one `Server`
runs at a time. The embedding program reads and feeds imaged through the same
handle: `imaged.import_archive(reader)` imports an image archive and returns
the image, `imaged.script_inputs(image_id)` lists the inputs declared by the
image's scripts in script order (both return `io::Result`, with `NotFound`,
`InvalidInput` and `AlreadyExists` kinds), `imaged.connected_hosts()` lists the
ids of the machines whose agent is connected and `imaged.task_progress()`
returns the progress of every task being worked on, the same `TaskProgress` the
dashboard shows. `imaged.watch_task_progress()` follows it: the feed's `next()`
returns the current progress at once and then waits for each change; changes
made between two calls are merged, so a slow reader only sees the latest.
`Image`, `TaskProgress` and `ScriptInput` are re-exported from `imaged_server`.

The server side (core, boot, both API crates and the `server` library) also builds
for Windows (`x86_64-pc-windows-gnu`). Multicast is Linux-only: on other
platforms a multicast task fails with "multicast is only supported on Linux".

Run (dev, see [Dev environment](#dev-environment)):

```sh
dx serve --package imaged-server --addr 0.0.0.0 --port 8080
```

Bundle (production; generate `crates/web/assets/tailwind.css` first, the dev
shell does it on entry). The bundle is a `server` binary with a `public/`
directory next to it; `nix build` does all of this:

```sh
dx bundle --release --platform web --package imaged-server
```

The binary requires a config file, `--config <FILE>` (or `IMAGED_CONFIG`).
Unknown keys and invalid values are errors; every error is printed as
`imaged-server: <message>` with exit code 1. SIGINT and SIGTERM both stop the
server gracefully. Keys ending in `_file` or `_file_contents` are replaced by
the content of the named file, relative to the config file, under the name
without the suffix ([toml-file-refs](https://crates.io/crates/toml-file-refs)):
`_file` takes a single line (one trailing newline is stripped, any other line
break is an error), `_file_contents` takes the file verbatim, so multi-line
secrets need it.

```toml
log_level = "info"                    # off, error, warn, info, debug or trace; default "info"
data_dir = "/var/lib/imaged"          # database and images, relative to the working directory; default "."
multicast_interface = "eth0"          # default "lo"
public_url = "http://10.0.0.1:8080"   # required; how booting machines reach `listen.machine`
script_proxy = "http://10.0.0.1:3128" # optional

[listen]
machine = "0.0.0.0:8080"              # PXE and agent API; default "0.0.0.0:8080", under `dx serve` dx's backend address
dashboard = "127.0.0.1:8081"          # dashboard and /api/ui: an address, "machine" or "off"; default "127.0.0.1:8081"
tftp = "0.0.0.0:69"                   # optional built-in TFTP server
proxy = "192.168.100.1:3128"          # optional HTTP proxy for post-install scripts

[script_variables]                    # every entry is handed to every post-install script
NTP_SERVER = "10.0.0.1"
SITE_NAME_file = "/etc/imaged/site-name"
AGE_KEY_file_contents = "/etc/imaged/age-key"
```

`log_level` applies to imaged's own crates; everything else logs at `warn`.
A set `RUST_LOG` replaces the whole filter. The NixOS module writes the file
from its options.

### `crates/agent` — `imaged-agent`

Rust binary that runs on a PXE-booted machine. On start it picks the disk to
image and connects to `/api/client/stream` over a websocket with its MAC
address (lowercase, colon-separated) and either the disk's size or why it has
no usable one (no disk, or more than one candidate). A host without a usable
disk still registers and the dashboard shows the reason; capture, deploy and
multicast to it are refused. Over the websocket the server names the host's
current task: an idle agent starts it, a running task that is no longer
current is cancelled, and an agent that becomes idle says so and gets the next
task, so a task queued while the agent was busy is delivered when it is done.
The agent reconnects after 15 seconds without hearing from the server. It
uploads and downloads partition tables (with their sector size), partitions
and script output over HTTP, and retries its final report with backoff while
the server is unreachable or answers 5xx. Task progress goes up the same
websocket: the agent weighs each partition by its size (swap counts as nothing,
only its header is imaged) and sends its latest progress at most every 5
seconds, only when it changed, and again after a reconnect. Uses `imaged-disk`
(`partclone` for filesystem-aware imaging, `sgdisk` for the partition table,
`debugfs` for the root partition). A capture runs `e2fsck -p` or `fsck.fat -a`
on each partition right before cloning it. Reboot tasks touch no disk.

On the machine's screen the agent draws a dashboard on tty1 (the initramfs
starts it there with `openvt` and quiets kernel messages to errors). F1 shows
the progress: the whole task, the current rate and time left in big digits,
every partition and post-install script, and the latest logs. A multicast
shows panels coloured green, yellow or red (blue while waiting) for the
receiver's own state: scuttlecast's diagnosis (`DISK SLOW` when the disk cannot
keep up), receive rate with its history, disk write rate and stalls, loss,
NAKs and kernel drops. F2 shows the logs, F3 switches to a shell on tty2
(Alt+F1 comes back) and F10 reboots after a confirmation; a reboot during a
task first reports it failed. When a task ends a banner replaces the big
digits and the stats stay on screen; after a deploy, multicast or reboot task
the machine reboots after a 5 second countdown that any key cancels. Plain
logs, and anything written to stderr, go to the serial console. The dashboard only uses characters of
the kernel console font (code page 437) and its 8 colours, and fits down to
80x25.

```sh
cargo run -p imaged-agent -- http://<server>:8080            # dashboard in this terminal
cargo run -p imaged-agent -- --no-tui http://<server>:8080   # plain logs, Ctrl-S opens a shell
```

### `crates/pack` — `imaged-pack`

Rust binary that turns a raw GPT disk image file into a `.imaged` archive
without root, KVM or a PXE boot. The archive matches what a capture produces,
so it imports through `POST /api/ui/images/import` and deploys like a captured
image.

```
imaged-pack <DISK> --name <NAME> [--root-partition <N>] [--script <PATH>]... (-o|--output) <OUT>
```

It backs up the partition table with `sgdisk` into `parttable.bin` and takes
the partitions and sector size from that GPT backup, by the same rule as a
capture (only GPT disks; anything else fails with `only GPT disks are
supported, found <label>`); an image file counts as a disk with 512-byte
sectors. Each partition is copied into a sparse temp file, its filesystem read
with `blkid -p`, and cloned with `imaged-disk` into `p-<n>.pcl`. Unformatted
partitions are skipped like a capture skips them; ext2/3/4, vfat and swap are
packed; any other filesystem fails the run, and so does a disk without any
packable partition (`the manifest lists no partitions`).
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

The nix package brings its runtime tools (`sgdisk`, `blkid`, `debugfs`,
`partclone`).

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

Entering the dev shell, from any directory of the checkout, works on the
repository root: it copies the kernels and iPXE binaries into `assets/`,
regenerates `crates/web/assets/tailwind.css` (the dashboard does not compile
without it) and sets `IMAGED_CONFIG` to `dev/imaged.toml`. `imaged-boot`
embeds the boot assets from `assets/` (see above) and `build-initramfs` writes
the initramfs there from the working tree.

The shell also sets `DATABASE_URL=sqlite://imaged.db`, so sqlx checks its
queries against `imaged.db` at the repository root, which a fresh checkout
does not have. Either build with `SQLX_OFFLINE=true` (the checked-in
`crates/core/.sqlx`) or create the database first. On a fresh checkout:

```sh
nix develop
build-initramfs x86_64 arm64
sqlx database create && sqlx migrate run --source crates/core/migrations
cargo build
```

After changing a query, update `.sqlx` with
`cargo sqlx prepare -- --all-targets --features test-support` in `crates/core`.

Tests run in one pass:

```sh
cargo test --workspace
```

CI runs the same suite as `nix build .#checks.x86_64-linux.tests`, which
embeds placeholder boot assets so it builds neither the agent nor the
initramfs.

For machines and VMs on the netboot bridge, run the server with
`dx serve --package imaged-server --addr 0.0.0.0 --port 8080`: dx binds
127.0.0.1 by default, and `dev/imaged.toml` and `dev/dnsmasq.conf` point
machines at `http://192.168.100.1:8080`. Leave `listen.machine` unset under dx;
the server then runs on dx's backend address behind dx's proxy. The dev config
sets `listen.dashboard = "machine"`, so the dashboard is at
`http://localhost:8080`.

## Nix packages and NixOS

`nix build` builds `imaged-server` (the default package: binary plus the
dashboard's `public/` next to it); `.#imaged-pack`, `.#imaged-agent` and
`.#boot-assets` build the rest. `overlays.default` adds `imaged-server` and
`imaged-pack` to nixpkgs.

`nixosModules.default` runs the server as a hardened systemd service:

```nix
services.imaged.server = {
  enable = true;
  publicUrl = "http://192.168.100.1:8080";   # required
  multicastInterface = "br-netboot";         # required
  bindAddress = "0.0.0.0:8080";              # default
  webBindAddress = "127.0.0.1:8081";         # default; also "machine" or "off"
  tftpBindAddress = "192.168.100.1:69";
  proxyBindAddress = "192.168.100.1:3128";
  scriptVariables.NTP_SERVER = "192.168.100.1";
  scriptVariableFiles.AGE_KEY = "/run/agenix/imaged-age-key";
  openFirewall = true;
};
```

`scriptVariables` end up in the Nix store; `scriptVariableFiles` are passed to
the service as systemd credentials, so the files can be readable by root only
(a name cannot be in both). `openFirewall` opens the machine, TFTP and proxy
ports on `multicastInterface`; a separate dashboard address is never opened,
but with `webBindAddress = "machine"` the dashboard shares the opened machine
port. The multicast UDP range 50000-50257 is always open there. `logLevel`
takes the `log_level` values and
`scriptProxy` sets `script_proxy`. The data dir (default `/var/lib/imaged`) is
`0700` and the service runs with `UMask=0077`, since the database holds every
deploy's input values. The service waits for `network-online.target`, which
does not cover a bridge that comes up later (such as a Docker network): it
restarts every 5 s until its addresses can be bound. `useBinaryCache = true`
makes the whole system trust the project's cachix cache
(`luukblankenstijn.cachix.org`); it is off by default.

## Network boot

DHCP hands bare PXE ROMs an iPXE binary over TFTP by client architecture
(option 93): `undionly.kpxe` for BIOS (0), `ipxe.efi` for x86 UEFI (6, 7, 9)
and `ipxe-arm64.efi` for arm64 UEFI (11). iPXE then chains
`/boot/boot.ipxe`, which reports its build architecture so the server answers
with the matching kernel and initramfs. See `dev/dnsmasq.conf` for a working
setup: `docker compose -f dev/compose.yaml up` (project `imaged`) runs it with
`assets/` mounted read-only, so build every initramfs first; a missing file
fails instead of turning into a root-owned directory.

`listen.tftp = "0.0.0.0:69"` also serves the iPXE binaries,
kernels and initramfs over TFTP, so no separate TFTP server is needed. It uses
a fixed 3 second retransmit timeout (at most 10 retries) and ignores the
client's `timeout` option.

A multicast sends each file to its own UDP port pair from 50000 up (the
partition table on 50000/50001, partition `n` on `50000 + 2n` and the next) in
an organisation-local group in 239.192.0.0/14, on `multicast_interface`.
Machines have 5 minutes to join. Open that port range on the netboot interface
and turn off IGMP snooping on bridges in between.

## Post-install scripts

Scripts are a library on the server (`/api/ui/scripts`, written with
`POST /api/ui/scripts/create`, `/update` and `/delete`): a unique name (letters,
digits, `-`, `_`, `.`), a body and a list of declared inputs. An input has a
name (`[a-z0-9_]+`), a label, an optional description (at most 1000
characters, shown under the field wherever its value is entered: the
dashboard's deploy and multicast dialogs), a kind (`text`, `number`, `choice`
with its choices, or `secret`), whether it is required, an optional default and
whether its value is given per machine or shared by every machine of a
deployment. Inputs have the same JSON shape in `/api/ui/scripts/*`, archive
manifests and `imaged-pack`. A `secret` input has no default and no choices;
the dashboard enters it in a masked multi-line field, and like every input
value it is stored with the task host and never returned by `/api/ui/*`, but it
is handed to whoever presents the host's MAC (see [Security](#security)). A
script that prints it puts it in the stored script output. An
image keeps an
ordered list of scripts (`POST /api/ui/images/scripts`) and always runs their
current version; a script that an image still uses cannot be deleted. Images
also carry a root partition (`POST /api/ui/images/root-partition`), the
partition the scripts run in. A capture finds it after cloning every partition
by reading each ext partition with `debugfs`, without mounting it: when exactly
one holds `/etc/os-release` (a symlink counts) it becomes the image's root
partition, otherwise it is left unset for the dashboard to choose.
`imaged-pack` applies the same rule.

Partitions without a filesystem, such as a BIOS boot partition, are not
imaged: a deploy recreates them in the partition table but writes nothing into
them. A captured machine that boots GRUB from BIOS therefore needs a
post-install script that reinstalls GRUB on the target disk. The NixOS image's
script does this; a captured non-NixOS machine needs its own.

A deploy (`POST /api/ui/hosts/deploy`, body
`{"req": {"id": <host>, "image_id": <image>, "inputs": {...}}}`) or multicast
(`POST /api/ui/groups/multicast`, body
`{"req": {"host_ids": [...], "image_id": <image>, "inputs": {...}}}`) of an
image with scripts takes the input values as
`inputs: { shared: { name: source }, per_machine: { "<host_id>": { name:
source } } }`: shared inputs come from `shared`, per-machine inputs from each
host's entry. A source is `{ "value": "…" }` for a typed value or
`{ "variable": "NAME" }` to take the value of a `[script_variables]` entry;
`GET /api/ui/scripts/variables` lists the variable names (never their
values), and the deploy and multicast dialogs offer them next to each field.
Unknown fields in the request or in `inputs` are refused (currently with a 500
naming the field). Before any task is created the sources are resolved and
checked (unknown variables, unknown or misplaced names, required values,
numbers, choices; errors name the host) and an empty value falls back to the
input's default; a rejected request is a 400, and an image with scripts but no
root partition is refused with a 412. The resolved values, secrets included,
are stored with each task host and kept indefinitely, so a retried or
reconnecting host gets the same ones, even if the variable changes later. If a
script has since changed so that the stored values no longer fit its inputs,
the restore stops with a 412 before anything is written; start a new
deployment.

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
its shebang or `/bin/sh` without one, in an empty environment with only these
variables:

- `PATH` (the usual `/usr/local/sbin` to `/bin` directories) and `HOME=/root`,
- `IMAGED_INPUT_<NAME>` for each of its inputs that has a value (the name
  uppercased),
- `IMAGED_MAC`, `IMAGED_IP` and `IMAGED_SERVER` of the agent,
- `http_proxy`, `https_proxy`, `HTTP_PROXY` and `HTTPS_PROXY` when
  `script_proxy` is set (`Config::script_proxy` for embedding programs).
- every entry of `[script_variables]` in the config file
  (`Config::script_variables`), for every script; names must be shell
  identifiers, cannot be one of the variables above and cannot start with
  `IMAGED_INPUT_`.

Stdout and stderr go to the agent log and, as the script runs, to the server,
which keeps the output and exit code of every run per task host
(`GET /api/ui/tasks/{task_id}/hosts/{host_id}/scripts`). The agent forwards the
first 1 MiB of a run live and then only the last 256 KiB; the server keeps the
first 64 KiB and the last 256 KiB, with a line saying how many bytes were
omitted in between. A run interrupted by a failed, cancelled or kicked host is
shown as finished without an exit code. A script that exits
non-zero stops the deploy and fails that host with the script's name; the
mounts are always undone in reverse order before the agent reports back and
reboots.

## Security

imaged has no authentication. Anyone who reaches the dashboard listener has
full control, and so does the whole netboot network when the dashboard is on
the machine listener; anyone on the netboot network can act as any host. Run
it on a trusted, isolated network.

- The dashboard and `/api/ui` let whoever reaches them capture, deploy (erasing
  disks), import and attach scripts that run as root on deployed machines and
  export images. `listen.dashboard` is `127.0.0.1:8081` by default; put it on
  an admin-only interface or behind an authenticating reverse proxy.
  `"machine"` (NixOS: `webBindAddress = "machine"`) puts it on the listener
  every booting machine reaches, which `openFirewall` opens.
- `/api/ui` refuses cross-site requests: any method other than GET, HEAD and
  OPTIONS gets a 403 unless `Sec-Fetch-Site` is `same-origin` or `none` or,
  without that header, `Origin` (when present) matches `Host`, and a 415 unless
  its body is `application/json` or `application/octet-stream`. Clients that
  send neither header, such as curl, pass the origin check but still need a
  JSON or octet-stream body (`curl -d` sends a form type and gets a 415).
- The machine listener (PXE and `/api/client`) cannot create tasks, but agents
  are identified only by their MAC (`X-Agent-Mac`). Whoever presents the MAC
  of a host while it runs a task gets that host's input values, `secret`
  inputs included, and every `[script_variables]` entry, can upload partition
  data for an image it is capturing and can report its tasks' outcome.
- Every `[script_variables]` entry is exported to every post-install script,
  including scripts that arrived in an imported archive.
- Resolved input values, secrets included, are stored in plain text in
  `imaged.db` indefinitely, so retries get the same values; images can hold
  password hashes and host keys. Keep the data dir private (the NixOS module
  does).
- The built-in proxy serves anyone who reaches its port.

## End-to-end VM test

```sh
nix develop .#e2e -c e2e/run.sh
```

Runs as your user inside user, network and pid namespaces; it needs `/dev/kvm`
and `/dev/net/tun`, and every boot asset in `assets/` (or `IMAGED_BOOT_ASSETS`)
with a real x86_64 initramfs built from this tree (`build-initramfs x86_64
arm64`; the server does not build without the arm64 one). The script:

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
   the same GPT, a clean fsck, the same files, the restored swap header, and
   `/etc/hostname` set by the script.

`E2E_FIRMWARE` picks `bios` or `uefi`, `E2E_OUT` the directory for logs, disks
and archives (default `e2e/out`); `CARGO_TARGET_DIR` and `IMAGED_BOOT_ASSETS`
are honoured, and the assets the script checks are the ones it compiles in.

```sh
nix develop .#e2e -c e2e/nixos.sh
```

tests the NixOS image module with the configurations in
`e2e/nixos/configurations.nix` (disko is pinned in the script). It checks that
every configuration under `invalid` fails to evaluate with its assertion, builds
the `imaged` image of each case, imports it into a server whose
`[script_variables]` holds an age key, deploys it to a blank 8 GiB disk with two
typed per-machine secrets and the shared one taken from the variable, boots the
disk and checks what the system reports: every secret with its owner, mode and
content and readable by its owner, the root filesystem grown to the disk, the
firmware, and for disko an active swap partition. `E2E_NIXOS_CASES` picks the
cases (default `sdboot:uefi grub-bios:bios grub-hybrid:bios grub-hybrid:uefi
disko:uefi disko-grub:bios`), `E2E_OUT` defaults to `e2e/out/nixos`.

## NixOS images

`nixosModules.image` adds an `imaged` variant to a NixOS configuration's
images, so the configuration builds into an archive that imports and deploys
like a captured image:

```sh
nixos-rebuild build-image --image-variant imaged --flake .#my-host
# or: nix build .#nixosConfigurations.my-host.config.system.build.images.imaged
```

```nix
{
  imports = [ imaged.nixosModules.image ];
  imaged.image.secrets.age_key = {
    path = config.sops.age.keyFile;
    description = "Key that decrypts this machine's sops secrets";
  };
  imaged.image.secrets.app_token = {
    path = "/var/lib/app/token";
    owner = "app";
    group = "app";
    mode = "0440";
  };
}
```

The disk comes from disko when the configuration has `disko.devices.disk`,
otherwise from nixpkgs' `make-disk-image` with an ESP (`efi`, the default), an
ESP and a BIOS boot partition (`hybrid`) or a BIOS boot partition only
(`legacy+gpt`), set with `imaged.image.partitionTableType`, then an ext4 root.
Without disko, importing the module also changes the host's own configuration,
so a later `nixos-rebuild` of a deployed machine matches its disk:
`fileSystems."/"` becomes `/dev/disk/by-label/nixos` (ext4, `autoResize`) and,
with an ESP, `/boot` becomes `/dev/disk/by-label/ESP`, both with `mkForce`,
replacing the ones from `hardware-configuration.nix`. Both builders install the
system in a VM during the build, which only sees the image file; disko builds a
raw image for the variant whatever `disko.imageBuilder.imageFormat` says. The
root partition grows to fill the target disk on first boot
(`boot.growPartition`, `autoResize`). `imaged-pack` then packs the disk inside
the build, always with `--root-partition` (2 for `efi` and `legacy+gpt`, 3 for
`hybrid`, the root partition's number with disko), after checking that the
system profile on the disk is the system the post-install script installs.
`imaged.image.name` (default `system.name`) is the image name; the archive file
is named after it, with characters not allowed in store paths replaced by `-`.

The archive carries one post-install script, `imaged-nixos-<hash>`, where
`<hash>` is the first 12 characters of the system's store hash, so every build
has its own script and an import of another build never collides with one an
older image uses; old ones stay in the library until deleted. The script writes every
secret and installs the bootloader with the system's own
`system.build.installBootLoader`; it does not run activation. Each attribute of
`imaged.image.secrets` becomes a required, masked `secret` input of that script
(`label`, `description`, `perMachine` default true, `required` default true),
and the deploy writes its value once, verbatim, to `path` with `owner`, `group`
and `mode` (default `root`, `root`, `0400`); the value never enters the Nix
store or the archive. A boot loader device (GRUB's `devices`, Limine's BIOS
`biosDevice`) that does not exist on the deploying machine (such as
`/dev/disk/by-id/...` of the build host) is linked to the target disk; one that
exists and is another disk stops the deploy. disko with GRUB and a BIOS boot
partition works the same way: the image keeps the configured GRUB devices and
the builder links them to its build disk. With an EFI boot loader the script
stops the deploy when the ESP is not mounted.

Evaluation fails when imaged cannot deploy the configuration:

- disko with more than one disk, LVM, mdadm, ZFS or bcachefs devices, a table
  other than GPT, a filesystem other than ext2/3/4, vfat or swap, or a root
  partition that is not the last one (`size = "100%"`) or no partitions;
- without disko, block-device `fileSystems` other than `/` and, with an ESP,
  `/boot`, or block-device `swapDevices` (a swap file is fine);
- `system.etc.overlay` (also set by the perlless profile): the deploy reads
  `/etc/fstab` and writes secrets on the root filesystem, which the overlay
  hides at boot;
- an EFI boot loader (systemd-boot, GRUB or Limine with EFI support, rEFInd)
  whose `boot.loader.efi.efiSysMountPoint` no `fileSystems` entry mounts;
- a secret name that is not `[a-z0-9_]+`, a relative path, a path under `/run`,
  `/tmp`, `/dev`, `/proc`, `/sys`, `/nix/store` or a tmpfs mount, or a path
  that `environment.etc` also manages;
- an `owner` or `group` other than `root` that is not declared in `users.users`
  or `users.groups`, or one at all when `services.userborn` or
  `systemd.sysusers` creates users at boot: the deploy runs before the first
  boot, so it can only chown to users the build already created.

## Upgrading to 1.0

- The server needs a config file (`--config` or `IMAGED_CONFIG`) with
  `public_url`; there is no longer a defaults-only mode. The dashboard moved
  off the machine listener to `127.0.0.1:8081`; set
  `listen.dashboard = "machine"` for the old layout. In the NixOS module
  `publicUrl` and `multicastInterface` have no default, `webBindAddress`
  defaults to `127.0.0.1:8081` and the cachix cache needs `useBinaryCache`.
- Archives are format 2. Format 1 archives are refused; re-pack them with the
  new `imaged-pack`. Images already on the server are recorded with 512-byte
  sectors and export as format 2; they keep working unless they were captured
  from a disk with 4096-byte sectors, which must be captured again.
- Captures and `imaged-pack` refuse non-GPT disks.
- `/api/ui`: `GET /api/ui/tasks` returns `{"tasks", "next_cursor"}` pages
  instead of an array; `POST /api/ui/images/create` returns
  `{"image", "task"}`; multicast and reboot return the created task; deploy
  and multicast refuse unknown fields and images that are not `ready`; an
  image cannot be deleted while it is deployed. See
  [`crates/api/ui`](#cratesapiui--imaged-api-ui).
- The agent protocol changed: let running tasks finish and reboot machines
  still running an agent from an older server. Stored MACs are lowercased on
  upgrade and hosts whose MACs differed only in case or separator are merged.
- The database migrates forward on start and cannot be downgraded. Back it up
  with the server stopped or with `sqlite3 imaged.db .backup <file>`.

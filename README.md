# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then re-deploy it to other machines. Hosts boot over PXE into a small
client that talks to a central server. Inspired by the capture/deploy part of [The Fog Project](https://github.com/FOGProject/fogproject); multicast support is planned.

## Components

### `crates/core` — `imaged-core`

Rust library. Domain model, SQLite (sqlx) repositories for hosts, images and
tasks, the in-memory host connection registry, the multicast manager, image
storage, DI wiring, and the PXE routes. Image partitions are stored on disk
under `images/`.

### `crates/web` — `imaged-web`

Dioxus fullstack library that renders the dashboard (hydrated wasm client) and
mounts the server-side routes.

### `crates/api/ui` — `imaged-api-ui`

Dashboard server functions served under `/api/ui/*`.

### `crates/api/client` — `imaged-api-client`

Agent-facing server functions served under `/api/client/*`.

### `crates/server` — `imaged-server`

Binary package that wires `imaged-core`, `imaged-web` and the two API crates
together behind axum. By default it serves the dashboard and the agent API from
a single binary; `--web-bind-address` splits the dashboard UI/API and the agent
API onto two separate sockets. Styled with Tailwind CSS v4.

Run (dev):

```sh
dx serve --package imaged-server
```

Bundle (production):

```sh
dx bundle --release --platform web --package imaged-server
```

Listens on `0.0.0.0:8080` (`--bind-address`); an optional `--web-bind-address`
serves the dashboard on a separate socket.

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

It also provides everything else the project needs: a minimal Linux kernel,
partclone, udp-cast, the initramfs packages and scripts, and the test-VM
scripts.

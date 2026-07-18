# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then re-deploy it to other machines. Hosts boot over PXE into a small
client that talks to a central server. Inspired by the capture/deploy part of [The Fog Project](https://github.com/FOGProject/fogproject); multicast support is planned.

## Components

### `crates/server` — `imaged-server-core`

Rust library (axum). Domain model, SQLite (sqlx) repositories for hosts, images
and tasks, the in-memory host connection registry, the multicast manager, image
storage, and the PXE / agent HTTP routers. Image partitions are stored on disk
under `images/`.

### `crates/web` — `imaged-server`

Dioxus fullstack app; its binary is named `imaged-server`. It renders the
dashboard (hydrated wasm client) and hosts the dashboard server functions plus
the PXE / agent HTTP API in a single binary, backed by `imaged-server-core`.
Styled with Tailwind CSS v4.

Run (dev):

```sh
dx serve --package imaged-web
```

Bundle (production):

```sh
dx bundle --release --platform web --package imaged-web
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

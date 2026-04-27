# imaged

A self-hosted disk imaging tool: capture an image from a machine over the
network, then re-deploy it to other machines. Hosts boot over PXE into a small
client that talks to a central server. This project is inspired by the capturing and deploying part of [The Fog Project](https://github.com/FOGProject/fogproject). Multicast support is planned as well. The web-frontend is implemented by AI, all rust code is hand written.

## Components

### `crates/server` — `imaged-server`

Rust (axum + tonic). Serves a gRPC/gRPC-Web API consumed by the web-client and http api for the pxe client. Uses SQLite (sqlx) for hosts, images, and tasks. Stores image partitions on disk under `images/`.

Run:

```sh
cargo run -p imaged-server
```

Listens on `0.0.0.0:8080`.

### `crates/client` — `imaged-client`

Rust binary that runs on a PXE-booted machine. On start it sends its state to
the server (mac address, disk size) and processes capture / deploy tasks
issued back over sse's. Uses `partclone` for filesystem-aware imaging.

```sh
cargo run -p imaged-client -- http://<server>:8080
```

### `dashboard/` — web UI

React + Vite + TypeScript. Connects to the server via gRPC-Web through a
`/api` proxy. Manages hosts, images, and tasks.

```sh
cd dashboard
pnpm install
pnpm dev          # http://localhost:5173, proxies /api to localhost:8080
pnpm build        # production bundle in dist/
```

### `proto/` and `gen/`

Protocol definitions live in `proto/`. Code generation is driven by
`buf.gen.yaml`:

```sh
buf generate
```

This regenerates `gen/rs` (Rust, used by both client and server through
`imaged-rpc`) and `gen/ts` (TypeScript, used by the dashboard).

## Dev environment

A `flake.nix` provides the toolchain (Rust, Node/pnpm, buf, protoc,
sqlx-cli). With direnv:

```sh
direnv allow
```

It also provides all tools needed by the project. A minimal linux kernel, partclone,
udp-cast, several packages for in the initramfs and scripts to build the initramfs
and run the test vm.

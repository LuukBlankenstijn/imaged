{
  lib,
  craneLib,
  pkg-config,
  sqlite,
  dioxus-cli,
  tailwindcss_4,
  wasm-bindgen-cli,
  binaryen,
  initramfs,
}:

let
  # pxe.rs embeds assets/vmlinuz via include_bytes!, sqlx::migrate! reads
  # crates/core/migrations, the offline query cache lives in crates/core/.sqlx,
  # tailwind reads crates/web/input.css and dx reads crates/server/Dioxus.toml,
  # so the cargo source filter has to let those through.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources ../..)
      ../../assets/vmlinuz
      ../../crates/core/migrations
      ../../crates/core/.sqlx
      ../../crates/web/input.css
      ../../crates/server/Dioxus.toml
    ];
  };

  commonArgs = {
    pname = "imaged-server";
    version = "0.1.0";
    inherit src;
    strictDeps = true;

    nativeBuildInputs = [
      dioxus-cli
      tailwindcss_4
      wasm-bindgen-cli
      binaryen
      pkg-config
    ];

    buildInputs = [ sqlite ];

    SQLX_OFFLINE = "true";
  };
in
craneLib.mkCargoDerivation (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly (
      commonArgs
      // {
        cargoExtraArgs = "-p imaged-server";
        doCheck = false;
      }
    );

    postPatch = ''
      install -Dm0644 ${initramfs} assets/initramfs.cpio.gz
    '';

    buildPhaseCargoCommand = ''
      export HOME=$TMPDIR
      tailwindcss -i crates/web/input.css -o crates/web/assets/tailwind.css
      dx bundle --release --platform web --package imaged-server --out-dir "$TMPDIR/bundle"
    '';

    installPhaseCommand = ''
      install -Dm0755 "$TMPDIR/bundle/server" "$out/bin/imaged-server"
      cp -r "$TMPDIR/bundle/public" "$out/bin/public"
    '';

    meta = {
      description = "imaged server backend and Dioxus dashboard";
      homepage = "https://github.com/luuk/imaged";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  }
)

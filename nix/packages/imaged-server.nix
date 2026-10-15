{
  lib,
  stdenv,
  craneLib,
  pkg-config,
  sqlite,
  dioxus-cli,
  tailwindcss_4,
  wasm-bindgen-cli,
  binaryen,
  bootAssets,
}:

let
  # sqlx::migrate! reads crates/core/migrations, the offline query cache lives
  # in crates/core/.sqlx, tailwind reads crates/web/input.css and dx reads
  # crates/server/Dioxus.toml, so the cargo source filter has to let those
  # through. imaged-boot embeds the boot assets from IMAGED_BOOT_ASSETS.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources ../..)
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
    # The two cargo invocations `dx bundle --release` makes, so dx finds every
    # dependency of the server and the wasm client already compiled.
    cargoArtifacts = craneLib.buildDepsOnly (
      commonArgs
      // {
        doCheck = false;
        buildPhaseCargoCommand = ''
          cargo build --locked -p imaged-server --bin imaged-server \
            --no-default-features --features server \
            --target ${stdenv.hostPlatform.rust.rustcTarget} --profile server-release \
            --config 'profile.server-release.inherits="release"' \
            --config profile.server-release.strip=false
          RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
          cargo build --locked -p imaged-server --bin imaged-server \
            --no-default-features --features web \
            --target wasm32-unknown-unknown --profile wasm-release \
            --config 'profile.wasm-release.inherits="release"' \
            --config profile.wasm-release.strip=false \
            --config 'profile.wasm-release.opt-level="s"'
        '';
      }
    );

    IMAGED_BOOT_ASSETS = bootAssets;

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

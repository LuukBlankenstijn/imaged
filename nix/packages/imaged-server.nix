{
  lib,
  stdenv,
  craneLib,
  src,
  version,
  pkg-config,
  sqlite,
  dioxus-cli,
  tailwindcss_4,
  wasm-bindgen-cli,
  binaryen,
  bootAssets,
}:

let
  commonArgs = {
    pname = "imaged-server";
    inherit src version;
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

    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
      craneLib.removeReferencesToVendoredSourcesHook
      craneLib.removeReferencesToRustToolchainHook
    ];

    IMAGED_BOOT_ASSETS = bootAssets;

    doInstallCargoArtifacts = false;

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
      homepage = "https://github.com/LuukBlankenstijn/imaged";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
      mainProgram = "imaged-server";
    };
  }
)

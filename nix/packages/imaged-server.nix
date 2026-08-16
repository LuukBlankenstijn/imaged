{
  lib,
  stdenv,
  rustPlatform,
  rustToolchain,
  pkg-config,
  sqlite,
  dioxus-cli,
  tailwindcss_4,
  wasm-bindgen-cli,
  binaryen,
  initramfs,
}:

stdenv.mkDerivation {
  pname = "imaged-server";
  version = "0.1.0";

  src = ../..;

  cargoDeps = rustPlatform.importCargoLock {
    lockFile = ../../Cargo.lock;
  };

  nativeBuildInputs = [
    rustToolchain
    rustPlatform.cargoSetupHook
    dioxus-cli
    tailwindcss_4
    wasm-bindgen-cli
    binaryen
    pkg-config
  ];

  buildInputs = [
    sqlite
  ];

  SQLX_OFFLINE = "true";

  postPatch = ''
    install -Dm0644 ${initramfs} assets/initramfs.cpio.gz
  '';

  buildPhase = ''
    runHook preBuild
    export HOME=$TMPDIR
    tailwindcss -i crates/web/input.css -o crates/web/assets/tailwind.css
    dx bundle --release --platform web --package imaged-server --out-dir "$TMPDIR/bundle"
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm0755 "$TMPDIR/bundle/server" "$out/bin/imaged-server"
    cp -r "$TMPDIR/bundle/public" "$out/bin/public"
    runHook postInstall
  '';

  meta = {
    description = "imaged server backend and Dioxus dashboard";
    homepage = "https://github.com/luuk/imaged";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}

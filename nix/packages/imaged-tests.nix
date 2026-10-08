{
  lib,
  craneLib,
  pkg-config,
  sqlite,
  tailwindcss_4,
  util-linux,
  bootAssets,
  ipxeAssets,
}:

let
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

  # find_target_disk probes the machine's real block devices, which the build
  # sandbox has no /sys to expose. It stays in the suite for dev runs; here
  # there is nothing for it to inspect.
  agentArgs = {
    pname = "imaged-agent-tests";
    version = "0.1.0";
    src = craneLib.cleanCargoSource ../..;
    strictDeps = true;
    cargoExtraArgs = "-p imaged-client";
    nativeBuildInputs = [ util-linux ];
    cargoTestExtraArgs =
      "-- --skip find_target_disk_only_returns_an_imaging_candidate_without_root";
  };

  # The dioxus server-fn macro compiles either an HTTP client call or a direct
  # in-process call depending on imaged-api-client/server, and cargo unifies
  # features per invocation, so the agent gets a second, server-free pass.
  # These two mirror the test-native and test-agent aliases in .cargo/config.toml.
  nativeArgs = {
    pname = "imaged-native-tests";
    version = "0.1.0";
    inherit src;
    strictDeps = true;
    cargoExtraArgs =
      "--workspace --exclude imaged-client "
      + "--features imaged-api-client/server,imaged-api-ui/server,imaged-server/server";
    nativeBuildInputs = [
      pkg-config
      tailwindcss_4
    ];
    buildInputs = [ sqlite ];
    SQLX_OFFLINE = "true";
  };

  # pxe.rs embeds the kernels and initramfs, tftp.rs embeds the iPXE
  # bootloaders, and web/app.rs references the stylesheet through asset! -- all
  # resolved at compile time. None are tracked, and none exist in the dummy tree
  # crane builds deps against, so they are materialised for the test build only.
  realSourcePrep = ''
    mkdir -p assets
    cp -rL --no-preserve=mode ${bootAssets}/. ${ipxeAssets}/. assets/
    tailwindcss -i crates/web/input.css -o crates/web/assets/tailwind.css
  '';
in
{
  agent = craneLib.cargoTest (
    agentArgs
    // {
      cargoArtifacts = craneLib.buildDepsOnly (agentArgs // { doCheck = false; });
    }
  );

  native = craneLib.cargoTest (
    nativeArgs
    // {
      cargoArtifacts = craneLib.buildDepsOnly (nativeArgs // { doCheck = false; });
      postPatch = realSourcePrep;
    }
  );
}

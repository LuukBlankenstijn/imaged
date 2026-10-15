{
  lib,
  craneLib,
  pkg-config,
  sqlite,
  tailwindcss_4,
  util-linux,
  bootAssets,
  testTools,
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
  commonArgs = {
    pname = "imaged-tests";
    version = "0.1.0";
    inherit src;
    strictDeps = true;
    cargoExtraArgs = "--workspace";
    cargoTestExtraArgs =
      "-- --skip find_target_disk_only_returns_an_imaging_candidate_without_root";
    nativeBuildInputs = [
      pkg-config
      tailwindcss_4
      testTools
      util-linux
    ];
    buildInputs = [ sqlite ];
    SQLX_OFFLINE = "true";
  };

  # web/app.rs references the stylesheet through asset!, resolved at compile
  # time. It is not tracked and does not exist in the dummy tree crane builds
  # deps against, so it is generated for the test build only.
  realSourcePrep = ''
    tailwindcss -i crates/web/input.css -o crates/web/assets/tailwind.css
  '';
in
craneLib.cargoTest (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly (
      commonArgs
      // {
        doCheck = false;
        buildPhaseCargoCommand = "cargoWithProfile test --workspace --no-run";
      }
    );
    postPatch = realSourcePrep;
    IMAGED_BOOT_ASSETS = bootAssets;
  }
)

{
  craneLib,
  src,
  version,
  pkg-config,
  sqlite,
  tailwindcss_4,
  util-linux,
  bootAssets,
  testTools,
}:

let
  commonArgs = {
    pname = "imaged-tests";
    inherit src version;
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

  generateTailwindCss = ''
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
    postPatch = generateTailwindCss;
    IMAGED_BOOT_ASSETS = bootAssets;
    doInstallCargoArtifacts = false;
  }
)

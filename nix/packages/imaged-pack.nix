{
  lib,
  craneLib,
  version,
  makeWrapper,
  imagingTools,
}:

let
  commonArgs = {
    pname = "imaged-pack";
    inherit version;
    src = craneLib.cleanCargoSource ../..;
    strictDeps = true;
    cargoExtraArgs = "-p imaged-pack";
    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly (
      commonArgs // { buildPhaseCargoCommand = "cargoWithProfile build -p imaged-pack"; }
    );
    nativeBuildInputs = [ makeWrapper ];
    postInstall = ''
      wrapProgram $out/bin/imaged-pack --prefix PATH : ${lib.makeBinPath [ imagingTools ]}
    '';
    meta.mainProgram = "imaged-pack";
  }
)

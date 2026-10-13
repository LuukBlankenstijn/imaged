{
  lib,
  craneLib,
  makeWrapper,
  imagingTools,
}:

let
  commonArgs = {
    pname = "imaged-pack";
    version = "0.1.0";
    src = craneLib.cleanCargoSource ../..;
    strictDeps = true;
    cargoExtraArgs = "-p imaged-pack";
    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
    nativeBuildInputs = [ makeWrapper ];
    postInstall = ''
      wrapProgram $out/bin/imaged-pack --prefix PATH : ${lib.makeBinPath [ imagingTools ]}
    '';
    meta.mainProgram = "imaged-pack";
  }
)

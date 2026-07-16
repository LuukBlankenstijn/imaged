{
  lib,
  stdenv,
  nodejs,
  pnpm_10,
  fetchPnpmDeps,
  pnpmConfigHook,
}:

let
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../pnpm-lock.yaml
      ../../pnpm-workspace.yaml
      ../../dashboard
      ../../gen/ts
    ];
  };
  depsSrc = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../pnpm-lock.yaml
      ../../pnpm-workspace.yaml
      ../../dashboard/package.json
      ../../gen/ts/package.json
    ];
  };
in
stdenv.mkDerivation (finalAttrs: {
  pname = "imaged-dashboard";
  version = "0.1.0";

  inherit src;

  nativeBuildInputs = [
    nodejs
    (pnpmConfigHook.overrideAttrs (prev: {
      propagatedBuildInputs = (prev.propagatedBuildInputs or [ ]) ++ [ pnpm_10 ];
    }))
  ];

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version;
    src = depsSrc;
    pnpm = pnpm_10;
    fetcherVersion = 1;
    hash = "sha256-zdD+p6PWEfZlGzB2iz7fIcqduuja4CT/Kuepi4MBjSA=";
  };

  buildPhase = ''
    runHook preBuild
    pnpm --filter @imaged/dashboard build
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    cp -r dashboard/dist $out
    runHook postInstall
  '';

  meta = {
    description = "imaged dashboard frontend";
    homepage = "https://github.com/luuk/imaged";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
})

{
  lib,
  rustPlatform,
  pkg-config,
  sqlite,
  protobuf,
}:

rustPlatform.buildRustPackage {
  pname = "imaged-server";
  version = "0.1.0";

  src = ./..;

  cargoLock = {
    lockFile = ./../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
    protobuf
  ];

  buildInputs = [
    sqlite
  ];

  # Note: sqlx query! macros require a database at compile time.
  # If the .sqlx directory is missing, you may need to run 'cargo sqlx prepare' 
  # or provide a DATABASE_URL during build.
  # For now, we assume the environment is set up or query! macros are handled.
  
  buildAndTestSubdir = "crates/server";

  meta = with lib; {
    description = "imaged server backend";
    homepage = "https://github.com/luuk/imaged";
    license = licenses.mit;
    maintainers = [ ];
    platforms = platforms.linux;
  };
}

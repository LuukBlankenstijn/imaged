{
  pkgsStatic,
  fetchurl,
  m4,
  perl,
}:

pkgsStatic.stdenv.mkDerivation {
  pname = "udpcast";
  version = "20211207";
  src = fetchurl {
    url = "http://www.udpcast.linux.lu/download/udpcast-20211207.tar.gz";
    sha256 = "sha256-o86+56h+zxvKBkXxJb54+9ezeEak2oL+zvlrksxk0FA=";
  };
  postPatch = ''
    sed -i '1i #include <stddef.h>' receivedata.c
    sed -i '1i #include <stddef.h>' participants.c
  '';
  nativeBuildInputs = [
    m4
    perl
  ];
  makeFlags = [ "LDFLAGS=-static" ];
  installPhase = ''
    mkdir -p $out/bin
    cp udp-receiver udp-sender $out/bin/
  '';
}

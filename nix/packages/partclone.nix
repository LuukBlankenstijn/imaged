{ pkgsStatic, pkg-config }:

(pkgsStatic.partclone.override {
  nilfs-utils = null;
}).overrideAttrs
  (oldAttrs: {
    configureFlags = [
      "--enable-extfs"
      "--enable-fat"
      "--enable-ntfs"
      "--enable-pkg-config-static"
      "--disable-xfs"
      "--disable-btrfs"
      "--disable-f2fs"
      "--disable-nilfs2"
      "--disable-minix"
      "--disable-hfsp"
    ];

    NIX_LDFLAGS = "-lext2fs -lcom_err -lblkid -luuid";

    buildInputs = oldAttrs.buildInputs ++ [
      pkgsStatic.e2fsprogs
      pkgsStatic.libuuid
    ];

    nativeBuildInputs = (oldAttrs.nativeBuildInputs or [ ]) ++ [
      pkg-config
    ];
  })

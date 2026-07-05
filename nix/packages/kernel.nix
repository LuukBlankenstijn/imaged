{
  stdenv,
  linux_latest,
  linuxManualConfig,
  perl,
  gnumake,
  bison,
  flex,
}:

let
  inherit (linux_latest) src version;

  configfile = stdenv.mkDerivation {
    name = "config";
    inherit src;
    nativeBuildInputs = [
      perl
      gnumake
      stdenv.cc
      bison
      flex
    ];

    buildPhase = ''
            patchShebangs scripts/config

            make x86_64_defconfig

            cat >> .config <<EOF
      CONFIG_IP_PNP=y
      CONFIG_IP_PNP_DHCP=y
      CONFIG_PACKET=y
      CONFIG_VIRTIO_NET=y
      CONFIG_VIRTIO_PCI=y
      CONFIG_E1000=y
      CONFIG_E1000E=y
      CONFIG_USB_RTL8152=y
      CONFIG_IKCONFIG=y
      CONFIG_IKCONFIG_PROC=y
      CONFIG_DEVTMPFS=y
      CONFIG_DEVTMPFS_MOUNT=y
      EOF

            ./scripts/config --enable CONFIG_IP_PNP_DHCP
            ./scripts/config --enable CONFIG_VIRTIO_NET
            ./scripts/config --enable CONFIG_E1000
            ./scripts/config --enable CONFIG_E1000E

            # Storage controllers built-in so real disks are detected.
            ./scripts/config --enable CONFIG_NVME_CORE
            ./scripts/config --enable CONFIG_BLK_DEV_NVME
            ./scripts/config --enable CONFIG_PCI_MSI
            ./scripts/config --enable CONFIG_VMD
            ./scripts/config --enable CONFIG_ATA
            ./scripts/config --enable CONFIG_SATA_AHCI
            ./scripts/config --enable CONFIG_ATA_PIIX
            ./scripts/config --enable CONFIG_SCSI
            ./scripts/config --enable CONFIG_BLK_DEV_SD
            ./scripts/config --enable CONFIG_MMC
            ./scripts/config --enable CONFIG_MMC_BLOCK
            ./scripts/config --enable CONFIG_MMC_SDHCI
            ./scripts/config --enable CONFIG_MMC_SDHCI_PCI
            ./scripts/config --enable CONFIG_MMC_SDHCI_ACPI

            # EFI framebuffer console for physical monitor output.
            ./scripts/config --enable CONFIG_SYSFB_SIMPLEFB
            ./scripts/config --enable CONFIG_DRM
            ./scripts/config --enable CONFIG_DRM_SIMPLEDRM
            ./scripts/config --enable CONFIG_DRM_FBDEV_EMULATION
            ./scripts/config --enable CONFIG_FB
            ./scripts/config --enable CONFIG_FB_EFI
            ./scripts/config --enable CONFIG_FRAMEBUFFER_CONSOLE

            make olddefconfig
    '';

    installPhase = "cp .config $out";
  };
in
linuxManualConfig {
  inherit src version configfile;
  allowImportFromDerivation = true;
}

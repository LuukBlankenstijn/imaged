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
      CONFIG_IKCONFIG=y
      CONFIG_IKCONFIG_PROC=y
      CONFIG_DEVTMPFS=y
      CONFIG_DEVTMPFS_MOUNT=y
      EOF

            ./scripts/config --enable CONFIG_IP_PNP_DHCP
            ./scripts/config --enable CONFIG_VIRTIO_NET
            ./scripts/config --enable CONFIG_E1000
            ./scripts/config --enable CONFIG_E1000E

            # USB networking, for machines with no built-in NIC that image over a
            # dongle. USB_NET_DRIVERS and USB_USBNET must be on or olddefconfig
            # silently drops every driver below them.
            ./scripts/config --enable CONFIG_USB_SUPPORT
            ./scripts/config --enable CONFIG_USB
            ./scripts/config --enable CONFIG_USB_PCI
            ./scripts/config --enable CONFIG_USB_XHCI_HCD
            ./scripts/config --enable CONFIG_USB_XHCI_PCI
            ./scripts/config --enable CONFIG_USB_EHCI_HCD
            ./scripts/config --enable CONFIG_USB_EHCI_PCI
            ./scripts/config --enable CONFIG_USB_NET_DRIVERS
            ./scripts/config --enable CONFIG_USB_USBNET
            ./scripts/config --enable CONFIG_USB_RTL8152
            ./scripts/config --enable CONFIG_USB_NET_AX8817X
            ./scripts/config --enable CONFIG_USB_NET_AX88179_178A
            ./scripts/config --enable CONFIG_USB_NET_CDCETHER
            ./scripts/config --enable CONFIG_USB_NET_CDC_NCM
            ./scripts/config --enable CONFIG_USB_NET_CDC_EEM
            ./scripts/config --enable CONFIG_USB_NET_SMSC75XX
            ./scripts/config --enable CONFIG_USB_NET_SMSC95XX
            ./scripts/config --enable CONFIG_USB_LAN78XX

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

            # olddefconfig drops symbols whose dependencies are unmet, so fail
            # the build here rather than at boot on a machine with no NIC.
            for sym in USB_USBNET USB_RTL8152 USB_NET_AX8817X \
                       USB_NET_AX88179_178A USB_NET_CDCETHER USB_XHCI_HCD; do
              grep -q "^CONFIG_$sym=y" .config || {
                echo "ERROR: CONFIG_$sym is not enabled in the final config" >&2
                exit 1
              }
            done
    '';

    installPhase = "cp .config $out";
  };
in
linuxManualConfig {
  inherit src version configfile;
  allowImportFromDerivation = true;
}

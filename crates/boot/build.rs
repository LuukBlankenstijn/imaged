use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

const ASSETS_ENV: &str = "IMAGED_BOOT_ASSETS";

const REQUIRED: [&str; 7] = [
    "ipxe.efi",
    "ipxe-arm64.efi",
    "undionly.kpxe",
    "x86_64/vmlinuz",
    "x86_64/initramfs.cpio.xz",
    "arm64/vmlinuz",
    "arm64/initramfs.cpio.xz",
];

fn main() -> ExitCode {
    println!("cargo:rerun-if-env-changed={ASSETS_ENV}");

    let default = default_dir();
    let (dir, source) = match env::var_os(ASSETS_ENV) {
        Some(dir) => (PathBuf::from(dir), ASSETS_ENV),
        None => (default.clone(), "the default <workspace root>/assets"),
    };
    if dir.is_relative() {
        eprintln!(
            "error: {ASSETS_ENV} must be an absolute path, got {}",
            dir.display()
        );
        return ExitCode::FAILURE;
    }

    for asset in REQUIRED {
        println!("cargo:rerun-if-changed={}", dir.join(asset).display());
    }

    let missing: Vec<PathBuf> = REQUIRED
        .iter()
        .map(|asset| dir.join(asset))
        .filter(|path| !path.is_file())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "error: imaged-boot embeds the boot assets at compile time, but the asset directory {} (from {source}) lacks:",
            dir.display()
        );
        for path in &missing {
            eprintln!("  {}", path.display());
        }
        eprintln!(
            "Either set {ASSETS_ENV} to an absolute directory holding {}, or put them in {} \
             (in the dev shell the shell hook copies the kernels and iPXE there and \
             `build-initramfs x86_64 arm64` builds the initramfs).",
            REQUIRED.join(", "),
            default.display()
        );
        return ExitCode::FAILURE;
    }

    println!("cargo:rustc-env=IMAGED_BOOT_ASSETS_DIR={}", dir.display());
    ExitCode::SUCCESS
}

fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/boot sits two levels below the workspace root")
        .join("assets")
}

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Response, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use derive_more::Display;
use serde::Deserialize;

static X86_64_VMLINUZ: &[u8] = include_bytes!("../../../assets/x86_64/vmlinuz");
static X86_64_INITRAMFS: &[u8] = include_bytes!("../../../assets/x86_64/initramfs.cpio.xz");
static ARM64_VMLINUZ: &[u8] = include_bytes!("../../../assets/arm64/vmlinuz");
static ARM64_INITRAMFS: &[u8] = include_bytes!("../../../assets/arm64/initramfs.cpio.xz");

#[derive(Clone, Copy, Deserialize, Display)]
pub(crate) enum Arch {
    #[serde(rename = "x86_64", alias = "i386")]
    #[display("x86_64")]
    X86_64,
    #[serde(rename = "arm64")]
    #[display("arm64")]
    Arm64,
}

impl Arch {
    fn serial_console(self) -> &'static str {
        match self {
            Arch::X86_64 => "ttyS0",
            Arch::Arm64 => "ttyAMA0",
        }
    }

    fn early_console(self) -> &'static str {
        match self {
            Arch::X86_64 => "earlyprintk=vga",
            Arch::Arm64 => "earlycon",
        }
    }

    pub(crate) fn vmlinuz(self) -> &'static [u8] {
        match self {
            Arch::X86_64 => X86_64_VMLINUZ,
            Arch::Arm64 => ARM64_VMLINUZ,
        }
    }

    pub(crate) fn initramfs(self) -> &'static [u8] {
        match self {
            Arch::X86_64 => X86_64_INITRAMFS,
            Arch::Arm64 => ARM64_INITRAMFS,
        }
    }
}

#[derive(Deserialize)]
pub struct BootParams {
    arch: Arch,
    product: Option<String>,
    manufacturer: Option<String>,
}

/// This ipxe script determines the machine type and chains into the manifest handler
pub async fn bootstrap_ipxe(State(base): State<Arc<str>>) -> impl IntoResponse {
    let body = format!(
        "#!ipxe\n\
         chain {base}/boot/manifest.ipxe?arch=${{buildarch}}&product=${{smbios/product:uristring}}&manufacturer=${{smbios/manufacturer:uristring}}\n"
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

pub async fn manifest_ipxe(
    State(base): State<Arc<str>>,
    Query(params): Query<BootParams>,
) -> impl IntoResponse {
    let product = params.product.as_deref().unwrap_or("");
    let manufacturer = params.manufacturer.as_deref().unwrap_or("");

    let is_vm = product.contains("QEMU")
        || product.contains("Standard PC")
        || manufacturer.contains("QEMU");

    let serial = format!("console={},115200n8", params.arch.serial_console());
    let console_args = if is_vm {
        format!("console=tty0 {serial}")
    } else {
        format!("{serial} console=tty0")
    };

    tracing::info!(target: "boot_config", console_args = %console_args, "Selected console line");

    let arch = params.arch;
    let early_console = arch.early_console();
    let body = format!(
        "#!ipxe\n\
         set base {base}\n\
         kernel ${{base}}/boot/{arch}/vmlinuz initrd=initramfs.cpio.xz img_srv=${{base}} {console_args} {early_console} loglevel=8\n\
         initrd ${{base}}/boot/{arch}/initramfs.cpio.xz\n\
         boot\n",
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

async fn serve_vmlinuz(Path(arch): Path<Arch>) -> impl IntoResponse {
    let vmlinuz = arch.vmlinuz();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, vmlinuz.len().to_string())
        .body(Body::from(vmlinuz))
        .unwrap()
}

async fn serve_initramfs(Path(arch): Path<Arch>) -> impl IntoResponse {
    let initramfs = arch.initramfs();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-xz")
        .header(header::CONTENT_LENGTH, initramfs.len().to_string())
        .body(Body::from(initramfs))
        .unwrap()
}

pub fn router(public_url: &str) -> Router {
    Router::new()
        .route("/boot/boot.ipxe", get(bootstrap_ipxe))
        .route("/boot/manifest.ipxe", get(manifest_ipxe))
        .route("/boot/{arch}/vmlinuz", get(serve_vmlinuz))
        .route("/boot/{arch}/initramfs.cpio.xz", get(serve_initramfs))
        .with_state(Arc::from(public_url.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const PUBLIC_URL: &str = "https://imaged.example:8443";

    async fn get(uri: &str) -> Response<Body> {
        router(PUBLIC_URL)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn manifest_body(uri: &str) -> String {
        let resp = get(uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn assert_common_ipxe_invariants(script: &str, arch: &str) {
        assert!(script.starts_with("#!ipxe\n"), "script: {script}");
        assert!(
            script.contains(&format!("set base {PUBLIC_URL}\n")),
            "missing public url: {script}"
        );
        assert!(
            script.lines().any(|l| l.trim() == "boot"),
            "missing boot line: {script}"
        );
        assert!(
            script.contains(&format!("/boot/{arch}/vmlinuz ")),
            "missing {arch} kernel url: {script}"
        );
        assert!(
            script.contains(&format!("/boot/{arch}/initramfs.cpio.xz\n")),
            "missing {arch} initramfs url: {script}"
        );
    }

    async fn assert_serves(uri: &str, content_type: &str, asset: &[u8]) {
        let resp = get(uri).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            content_type
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_LENGTH).unwrap(),
            asset.len().to_string().as_str()
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes == asset, "{uri} served a different asset");
    }

    #[tokio::test]
    async fn bootstrap_chains_into_the_manifest_at_the_public_url_with_the_ipxe_build_arch() {
        let script = manifest_body("/boot/boot.ipxe").await;
        assert!(
            script.contains(&format!(
                "chain {PUBLIC_URL}/boot/manifest.ipxe?arch=${{buildarch}}&"
            )),
            "{script}"
        );
    }

    #[tokio::test]
    async fn a_trailing_slash_on_the_public_url_does_not_double_the_path_separator() {
        let resp = router(&format!("{PUBLIC_URL}/"))
            .oneshot(
                Request::builder()
                    .uri("/boot/manifest.ipxe?arch=arm64")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let script = String::from_utf8(bytes.to_vec()).unwrap();
        assert_common_ipxe_invariants(&script, "arm64");
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_product() {
        let script = manifest_body("/boot/manifest.ipxe?arch=x86_64&product=QEMU%20Standard").await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_standard_pc_product() {
        let script =
            manifest_body("/boot/manifest.ipxe?arch=x86_64&product=Standard%20PC%20(i440FX)").await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_manufacturer() {
        let script = manifest_body("/boot/manifest.ipxe?arch=x86_64&manufacturer=QEMU").await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_physical_console_line_by_default() {
        let script = manifest_body("/boot/manifest.ipxe?arch=x86_64&product=Dell%20Inc.").await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=ttyS0,115200n8 console=tty0"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_boots_bios_ipxe_into_the_x86_64_kernel() {
        let script = manifest_body("/boot/manifest.ipxe?arch=i386&product=Dell%20Inc.").await;
        assert_common_ipxe_invariants(&script, "x86_64");
    }

    #[tokio::test]
    async fn manifest_boots_arm64_into_the_arm64_kernel_on_the_pl011_console() {
        let script = manifest_body("/boot/manifest.ipxe?arch=arm64&manufacturer=QEMU").await;
        assert_common_ipxe_invariants(&script, "arm64");
        assert!(
            script.contains("console=tty0 console=ttyAMA0,115200n8 earlycon "),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_rejects_an_unknown_arch() {
        let resp = get("/boot/manifest.ipxe?arch=riscv64").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_vmlinuz_returns_the_kernel_built_for_the_requested_arch() {
        assert_serves(
            "/boot/x86_64/vmlinuz",
            "application/octet-stream",
            X86_64_VMLINUZ,
        )
        .await;
        assert_serves(
            "/boot/arm64/vmlinuz",
            "application/octet-stream",
            ARM64_VMLINUZ,
        )
        .await;
    }

    #[tokio::test]
    async fn serve_initramfs_returns_the_initramfs_built_for_the_requested_arch() {
        assert_serves(
            "/boot/x86_64/initramfs.cpio.xz",
            "application/x-xz",
            X86_64_INITRAMFS,
        )
        .await;
        assert_serves(
            "/boot/arm64/initramfs.cpio.xz",
            "application/x-xz",
            ARM64_INITRAMFS,
        )
        .await;
    }
}

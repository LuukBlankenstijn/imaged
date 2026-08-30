use std::env;

use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{Response, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct BootParams {
    product: Option<String>,
    manufacturer: Option<String>,
}

fn get_dynamic_base() -> String {
    let proto = if env::var("USE_HTTPS").is_ok() {
        "https"
    } else {
        "http"
    };
    let public_base = env::var("PUBLIC_BASE").unwrap_or_else(|_| "localhost:8080".to_string());
    format!("{}://{}", proto, public_base)
}

/// This ipxe script determines the machine type and chains into the manifest handler
pub async fn bootstrap_ipxe() -> impl IntoResponse {
    let base = get_dynamic_base();

    let body = format!(
        "#!ipxe\n\
         chain {base}/boot/manifest.ipxe?product=${{smbios/product:uristring}}&manufacturer=${{smbios/manufacturer:uristring}}\n"
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

pub async fn manifest_ipxe(Query(params): Query<BootParams>) -> impl IntoResponse {
    let base = get_dynamic_base();

    let product = params.product.as_deref().unwrap_or("");
    let manufacturer = params.manufacturer.as_deref().unwrap_or("");

    let is_vm = product.contains("QEMU")
        || product.contains("Standard PC")
        || manufacturer.contains("QEMU");

    let console_args = if is_vm {
        "console=tty0 console=ttyS0,115200n8"
    } else {
        "console=ttyS0,115200n8 console=tty0"
    };

    tracing::info!(target: "boot_config", console_args = %console_args, "Selected console line");

    let body = format!(
        "#!ipxe\n\
         set base {base}\n\
         kernel ${{base}}/boot/vmlinuz initrd=initramfs.cpio.gz img_srv=${{base}} {console_args} earlyprintk=vga loglevel=8\n\
         initrd ${{base}}/boot/initramfs.cpio.gz\n\
         boot\n",
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

static VMLINUZ: &[u8] = include_bytes!("../../../assets/vmlinuz");
static INITRAMFS: &[u8] = include_bytes!("../../../assets/initramfs.cpio.gz");

async fn serve_vmlinuz() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        // .len() on a static slice is evaluated at compile time, so this is free
        .header(header::CONTENT_LENGTH, VMLINUZ.len().to_string())
        .body(Body::from(VMLINUZ))
        .unwrap()
}

async fn serve_initramfs() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gzip")
        .header(header::CONTENT_LENGTH, INITRAMFS.len().to_string())
        .body(Body::from(INITRAMFS))
        .unwrap()
}

pub fn router() -> Router {
    Router::new()
        .route("/boot/boot.ipxe", get(bootstrap_ipxe))
        .route("/boot/manifest.ipxe", get(manifest_ipxe))
        .route("/boot/vmlinuz", get(serve_vmlinuz))
        .route("/boot/initramfs.cpio.gz", get(serve_initramfs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    static VMLINUZ_ASSET: &[u8] = include_bytes!("../../../assets/vmlinuz");
    static INITRAMFS_ASSET: &[u8] = include_bytes!("../../../assets/initramfs.cpio.gz");

    async fn get(uri: &str) -> Response<Body> {
        router()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn manifest_body(uri: &str) -> String {
        let resp = get(uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn assert_common_ipxe_invariants(script: &str) {
        assert!(script.starts_with("#!ipxe\n"), "script: {script}");
        assert!(
            script.lines().any(|l| l.trim() == "boot"),
            "missing boot line: {script}"
        );
        assert!(script.contains("/boot/vmlinuz"), "missing kernel url: {script}");
        assert!(
            script.contains("/boot/initramfs.cpio.gz"),
            "missing initramfs url: {script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_product() {
        let script = manifest_body("/boot/manifest.ipxe?product=QEMU%20Standard").await;
        assert_common_ipxe_invariants(&script);
        assert!(script.contains("console=tty0 console=ttyS0,115200n8"), "{script}");
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_standard_pc_product() {
        let script = manifest_body("/boot/manifest.ipxe?product=Standard%20PC%20(i440FX)").await;
        assert_common_ipxe_invariants(&script);
        assert!(script.contains("console=tty0 console=ttyS0,115200n8"), "{script}");
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_manufacturer() {
        let script = manifest_body("/boot/manifest.ipxe?manufacturer=QEMU").await;
        assert_common_ipxe_invariants(&script);
        assert!(script.contains("console=tty0 console=ttyS0,115200n8"), "{script}");
    }

    #[tokio::test]
    async fn manifest_selects_the_physical_console_line_by_default() {
        let script = manifest_body("/boot/manifest.ipxe?product=Dell%20Inc.").await;
        assert_common_ipxe_invariants(&script);
        assert!(script.contains("console=ttyS0,115200n8 console=tty0"), "{script}");
    }

    #[tokio::test]
    async fn serve_vmlinuz_returns_octet_stream_with_content_length_matching_the_embedded_asset() {
        let resp = get("/boot/vmlinuz").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_LENGTH).unwrap(),
            VMLINUZ_ASSET.len().to_string().as_str()
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.len(), VMLINUZ_ASSET.len());
    }

    #[tokio::test]
    async fn serve_initramfs_returns_gzip_with_content_length_matching_the_embedded_asset() {
        let resp = get("/boot/initramfs.cpio.gz").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/gzip"
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_LENGTH).unwrap(),
            INITRAMFS_ASSET.len().to_string().as_str()
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.len(), INITRAMFS_ASSET.len());
    }
}

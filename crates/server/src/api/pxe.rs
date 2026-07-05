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

static VMLINUZ: &[u8] = include_bytes!("../../../../boot/vmlinuz");
static INITRAMFS: &[u8] = include_bytes!("../../../../boot/initramfs.cpio.gz");

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

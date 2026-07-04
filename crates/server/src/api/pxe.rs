use std::env;

use axum::{Router, response::IntoResponse, routing::get};
use tower_http::services::ServeDir;

pub async fn boot_ipxe() -> impl IntoResponse {
    let proto = if env::var("USE_HTTPS").is_ok() {
        "https"
    } else {
        "http"
    };
    let public_base = env::var("PUBLIC_BASE").unwrap_or("localhost:8080".to_string());
    let base = format!("{}://{}", proto, public_base);
    // TEMPORARY: ttyS0 is placed last so it becomes /dev/console and the client
    // output shows on the QEMU serial console (`run-vm-pxe` uses -nographic).
    // Restore `console=ttyS0,115200 console=tty0` (tty0 last) for physical
    // machines so their monitors show output.
    let body = format!(
        "#!ipxe\n\
         set base {base}\n\
         kernel ${{base}}/boot/vmlinuz initrd=initramfs.cpio.gz img_srv=${{base}} console=tty0 console=ttyS0,115200 earlyprintk=vga loglevel=8\n\
         initrd ${{base}}/boot/initramfs.cpio.gz\n\
         boot\n",
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

pub fn router() -> Router {
    Router::new()
        .route("/boot/boot.ipxe", get(boot_ipxe))
        .nest_service("/boot", ServeDir::new("./boot"))
}

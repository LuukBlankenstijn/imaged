use std::{
    collections::HashMap,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Path, RawQuery, State},
    http::{Response, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use derive_more::Display;
use futures_lite::stream;
use percent_encoding::percent_decode_str;
use serde::Deserialize;

use crate::{BootClient, BootFiles, SharedBootFiles, files::Profile};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Display)]
pub enum Arch {
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
            Arch::X86_64 => "ttyS0,115200n8",
            Arch::Arm64 => "ttyAMA0,115200n8",
        }
    }

    fn early_console(self) -> &'static str {
        match self {
            Arch::X86_64 => "earlyprintk=vga",
            Arch::Arm64 => "earlycon",
        }
    }
}

impl FromStr for Arch {
    type Err = ();

    fn from_str(arch: &str) -> Result<Self, ()> {
        match arch {
            "x86_64" | "i386" => Ok(Arch::X86_64),
            "arm64" => Ok(Arch::Arm64),
            _ => Err(()),
        }
    }
}

#[derive(Clone)]
struct Pxe {
    base: Arc<str>,
    files: SharedBootFiles,
}

fn boot_client(ip: IpAddr, query: &str) -> Option<BootClient> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(
            key,
            percent_decode_str(value).decode_utf8_lossy().into_owned(),
        );
    }
    let mut client = BootClient::new(params.get("arch")?.parse().ok()?, ip);
    client.mac = params.get("mac").and_then(|mac| mac.parse().ok());
    let mut text = |key| {
        params
            .remove(key)
            .filter(|value: &String| !value.trim().is_empty())
    };
    client.manufacturer = text("manufacturer");
    client.product = text("product");
    Some(client)
}

/// This ipxe script determines the machine type and chains into the manifest handler
async fn bootstrap_ipxe(State(Pxe { base, .. }): State<Pxe>) -> impl IntoResponse {
    let body = format!(
        "#!ipxe\n\
         chain {base}/boot/manifest.ipxe?arch=${{buildarch}}&mac=${{netX/mac}}&product=${{smbios/product:uristring}}&manufacturer=${{smbios/manufacturer:uristring}}\n"
    );

    ([("content-type", "text/plain; charset=utf-8")], body)
}

async fn manifest_ipxe(
    State(Pxe { base, files }): State<Pxe>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
) -> Response<Body> {
    let Some(client) = boot_client(peer.ip(), query.as_deref().unwrap_or("")) else {
        return (
            StatusCode::BAD_REQUEST,
            "manifest.ipxe needs arch: x86_64, i386 or arm64",
        )
            .into_response();
    };

    let product = client.product.as_deref().unwrap_or("");
    let manufacturer = client.manufacturer.as_deref().unwrap_or("");
    let is_vm = product.contains("QEMU")
        || product.contains("Standard PC")
        || manufacturer.contains("QEMU");

    let arch = client.arch;
    let files = files.current();
    let Some((name, profile)) = files.select(&client) else {
        return (
            StatusCode::NOT_FOUND,
            format!("this server has no {arch} boot files"),
        )
            .into_response();
    };
    let serial = format!(
        "console={}",
        profile.console.as_deref().unwrap_or(arch.serial_console())
    );
    let console_args = if is_vm {
        format!("console=tty0 {serial}")
    } else {
        format!("{serial} console=tty0")
    };
    tracing::info!(?client, profile = %name, console_args = %console_args, "boot profile selected");

    let early_console = arch.early_console();
    let body = format!(
        "#!ipxe\n\
         set base {base}\n\
         kernel ${{base}}/boot/profile/{name}/vmlinuz initrd=initramfs.cpio.xz img_srv=${{base}} {console_args} {early_console} loglevel=8\n\
         initrd ${{base}}/boot/profile/{name}/initramfs.cpio.xz\n\
         boot\n",
    );

    ([("content-type", "text/plain; charset=utf-8")], body).into_response()
}

fn selected<'a>(files: &'a BootFiles, arch: &str, peer: SocketAddr) -> Option<&'a Profile> {
    let client = BootClient::new(arch.parse().ok()?, peer.ip());
    let (name, profile) = files.select(&client)?;
    tracing::info!(?client, profile = %name, "boot profile selected for a fixed arch path");
    Some(profile)
}

fn kernel_response(profile: Option<&Profile>) -> Response<Body> {
    let Some(profile) = profile else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kernel = profile.kernel.clone();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, kernel.len().to_string())
        .body(Body::from(kernel))
        .unwrap()
}

fn initramfs_response(profile: Option<&Profile>) -> Response<Body> {
    let Some(profile) = profile else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let parts = profile
        .initramfs
        .clone()
        .into_iter()
        .map(Ok::<_, Infallible>);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, profile.initramfs_size().to_string())
        .body(Body::from_stream(stream::iter(parts)))
        .unwrap()
}

async fn arch_vmlinuz(
    State(pxe): State<Pxe>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(arch): Path<String>,
) -> Response<Body> {
    kernel_response(selected(&pxe.files.current(), &arch, peer))
}

async fn arch_initramfs(
    State(pxe): State<Pxe>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(arch): Path<String>,
) -> Response<Body> {
    initramfs_response(selected(&pxe.files.current(), &arch, peer))
}

async fn profile_vmlinuz(State(pxe): State<Pxe>, Path(name): Path<String>) -> Response<Body> {
    kernel_response(pxe.files.current().profile(&name))
}

async fn profile_initramfs(State(pxe): State<Pxe>, Path(name): Path<String>) -> Response<Body> {
    initramfs_response(pxe.files.current().profile(&name))
}

pub fn router(public_url: &str, files: SharedBootFiles) -> Router {
    Router::new()
        .route("/boot/boot.ipxe", get(bootstrap_ipxe))
        .route("/boot/manifest.ipxe", get(manifest_ipxe))
        .route("/boot/{arch}/vmlinuz", get(arch_vmlinuz))
        .route("/boot/{arch}/initramfs.cpio.xz", get(arch_initramfs))
        .route("/boot/profile/{profile}/vmlinuz", get(profile_vmlinuz))
        .route(
            "/boot/profile/{profile}/initramfs.cpio.xz",
            get(profile_initramfs),
        )
        .with_state(Pxe {
            base: Arc::from(public_url.trim_end_matches('/')),
            files,
        })
}

#[cfg(all(test, feature = "x86_64", feature = "arm64"))]
mod tests {
    use super::*;
    use crate::{
        ProfileName, ProfileSelector, ProfileSources, Sources,
        files::tests::{CLIENT_IP, Fixed, TempFile, name, profile_with},
    };
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const PUBLIC_URL: &str = "https://imaged.example:8443";

    async fn built_in() -> SharedBootFiles {
        SharedBootFiles::new(BootFiles::load(&Sources::default(), None).await.unwrap())
    }

    async fn get_from(router: Router, uri: &str) -> Response<Body> {
        let request = Request::builder()
            .uri(uri)
            .extension(ConnectInfo(SocketAddr::new(CLIENT_IP, 4011)))
            .body(Body::empty())
            .unwrap();
        router.oneshot(request).await.unwrap()
    }

    async fn get(uri: &str) -> Response<Body> {
        get_from(router(PUBLIC_URL, built_in().await), uri).await
    }

    async fn text(router: Router, uri: &str) -> String {
        let resp = get_from(router, uri).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
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
            script.contains(&format!("/boot/profile/{arch}/vmlinuz ")),
            "missing {arch} kernel url: {script}"
        );
        assert!(
            script.contains(&format!("/boot/profile/{arch}/initramfs.cpio.xz\n")),
            "missing {arch} initramfs url: {script}"
        );
    }

    async fn assert_serves(router: Router, uri: &str, asset: &[u8]) {
        let resp = get_from(router, uri).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
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
    async fn bootstrap_chains_into_the_manifest_with_the_build_arch_and_booting_interface() {
        let script = manifest_body("/boot/boot.ipxe").await;
        assert!(
            script.contains(&format!(
                "chain {PUBLIC_URL}/boot/manifest.ipxe?arch=${{buildarch}}&mac=${{netX/mac}}&"
            )),
            "{script}"
        );
    }

    #[tokio::test]
    async fn a_trailing_slash_on_the_public_url_does_not_double_the_path_separator() {
        let resp = get_from(
            router(&format!("{PUBLIC_URL}/"), built_in().await),
            "/boot/manifest.ipxe?arch=arm64&ip=192.168.213.100",
        )
        .await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let script = String::from_utf8(bytes.to_vec()).unwrap();
        assert_common_ipxe_invariants(&script, "arm64");
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_product() {
        let script = manifest_body(
            "/boot/manifest.ipxe?arch=x86_64&ip=192.168.213.100&product=QEMU%20Standard",
        )
        .await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_standard_pc_product() {
        let script = manifest_body(
            "/boot/manifest.ipxe?arch=x86_64&ip=192.168.213.100&product=Standard%20PC%20(i440FX)",
        )
        .await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_vm_console_line_for_a_qemu_manufacturer() {
        let script =
            manifest_body("/boot/manifest.ipxe?arch=x86_64&ip=192.168.213.100&manufacturer=QEMU")
                .await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=tty0 console=ttyS0,115200n8"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_selects_the_physical_console_line_by_default() {
        let script =
            manifest_body("/boot/manifest.ipxe?arch=x86_64&ip=192.168.213.100&product=Dell%20Inc.")
                .await;
        assert_common_ipxe_invariants(&script, "x86_64");
        assert!(
            script.contains("console=ttyS0,115200n8 console=tty0"),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_boots_bios_ipxe_into_the_x86_64_kernel() {
        let script =
            manifest_body("/boot/manifest.ipxe?arch=i386&ip=192.168.213.100&product=Dell%20Inc.")
                .await;
        assert_common_ipxe_invariants(&script, "x86_64");
    }

    #[tokio::test]
    async fn manifest_boots_arm64_into_the_arm64_kernel_on_the_pl011_console() {
        let script =
            manifest_body("/boot/manifest.ipxe?arch=arm64&ip=192.168.213.100&manufacturer=QEMU")
                .await;
        assert_common_ipxe_invariants(&script, "arm64");
        assert!(
            script.contains("console=tty0 console=ttyAMA0,115200n8 earlycon "),
            "{script}"
        );
    }

    #[tokio::test]
    async fn manifest_rejects_a_missing_or_unknown_arch() {
        for uri in [
            "/boot/manifest.ipxe?arch=riscv64",
            "/boot/manifest.ipxe?product=OptiPlex",
            "/boot/manifest.ipxe",
        ] {
            assert_eq!(get(uri).await.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
    }

    #[tokio::test]
    async fn profile_paths_serve_that_profile_and_arch_paths_ask_the_selector() {
        let kernel = TempFile::new("pxe-kernel", b"custom arm64 kernel");
        let extra = TempFile::new("pxe-extra-initrd", b"extra x86_64 initrd");
        let mut sources = Sources::default();
        sources
            .profiles
            .insert(name("arm64"), profile_with(Arch::Arm64, Some(&kernel), &[]));
        sources
            .profiles
            .insert(name("box"), profile_with(Arch::X86_64, None, &[&extra]));
        let shared = SharedBootFiles::new(
            BootFiles::load(&sources, Some(Arc::new(Fixed(Some("box")))))
                .await
                .unwrap(),
        );
        let files = shared.current();
        let router = router(PUBLIC_URL, shared);
        let box_initramfs = files.profile("box").unwrap().initramfs.concat();
        let x86_64_initramfs = files.profile("x86_64").unwrap().initramfs.concat();
        assert!(box_initramfs.ends_with(b"extra x86_64 initrd\0"));

        assert_serves(
            router.clone(),
            "/boot/profile/box/initramfs.cpio.xz",
            &box_initramfs,
        )
        .await;
        assert_serves(
            router.clone(),
            "/boot/profile/x86_64/initramfs.cpio.xz",
            &x86_64_initramfs,
        )
        .await;
        assert_serves(
            router.clone(),
            "/boot/x86_64/initramfs.cpio.xz",
            &box_initramfs,
        )
        .await;
        assert_serves(
            router.clone(),
            "/boot/arm64/vmlinuz",
            b"custom arm64 kernel",
        )
        .await;
        assert_serves(
            router.clone(),
            "/boot/profile/arm64/vmlinuz",
            b"custom arm64 kernel",
        )
        .await;
        for missing in [
            "/boot/profile/unknown/vmlinuz",
            "/boot/profile/a%20b/initramfs.cpio.xz",
            "/boot/box/vmlinuz",
            "/boot/riscv64/vmlinuz",
        ] {
            let resp = get_from(router.clone(), missing).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{missing}");
        }
    }

    #[tokio::test]
    async fn replaced_boot_files_are_served_from_the_next_request_on() {
        let kernel = TempFile::new("pxe-reload-kernel", b"reloaded kernel");
        let shared = built_in().await;
        let router = router(PUBLIC_URL, shared.clone());
        assert_eq!(
            get_from(router.clone(), "/boot/profile/board/vmlinuz")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let mut sources = Sources::default();
        sources
            .profiles
            .insert(name("board"), profile_with(Arch::Arm64, Some(&kernel), &[]));
        shared.replace(BootFiles::load(&sources, None).await.unwrap());

        assert_serves(router, "/boot/profile/board/vmlinuz", b"reloaded kernel").await;
    }

    #[tokio::test]
    async fn the_manifest_points_both_files_at_the_profile_the_selector_picked() {
        let mut sources = Sources::default();
        let mut jetson = ProfileSources::new(Arch::Arm64);
        jetson.console = Some("ttyTCU0,115200n8".to_string());
        sources.profiles.insert(name("jetson"), jetson);
        sources.products.insert(
            "NVIDIA Jetson Orin Nano Developer Kit".to_string(),
            name("jetson"),
        );
        let files = BootFiles::load(&sources, None).await.unwrap();
        let router = router(PUBLIC_URL, SharedBootFiles::new(files));

        let jetson = text(
            router.clone(),
            "/boot/manifest.ipxe?arch=arm64&ip=192.168.213.100&mac=48:b0:2d:aa:bb:cc&product=NVIDIA%20Jetson%20Orin%20Nano%20Developer%20Kit&manufacturer=NVIDIA",
        )
        .await;
        assert!(
            jetson.contains("kernel ${base}/boot/profile/jetson/vmlinuz "),
            "{jetson}"
        );
        assert!(
            jetson.contains("initrd ${base}/boot/profile/jetson/initramfs.cpio.xz\n"),
            "{jetson}"
        );
        assert!(
            jetson.contains("console=ttyTCU0,115200n8 console=tty0"),
            "{jetson}"
        );

        let other = text(
            router,
            "/boot/manifest.ipxe?arch=x86_64&ip=192.168.213.100&product=NVIDIA%20Jetson%20Orin%20Nano%20Developer%20Kit",
        )
        .await;
        assert_common_ipxe_invariants(&other, "x86_64");
    }

    #[tokio::test]
    async fn the_selector_sees_what_the_client_reported() {
        #[derive(Debug, Default)]
        struct Recording(std::sync::Mutex<Vec<BootClient>>);
        impl ProfileSelector for Recording {
            fn profile(&self, client: &BootClient) -> Option<ProfileName> {
                self.0.lock().unwrap().push(client.clone());
                None
            }
        }
        let selector = Arc::new(Recording::default());
        let files = BootFiles::load(&Sources::default(), Some(selector.clone()))
            .await
            .unwrap();
        let router = router(PUBLIC_URL, SharedBootFiles::new(files));

        text(
            router.clone(),
            "/boot/manifest.ipxe?arch=i386&ip=192.168.213.100&mac=52-54-00-AB-CD-EF&product=Standard%20PC%20(Q35%20+%20ICH9,%202009)&manufacturer=Dell%20Inc.%26Co",
        )
        .await;
        text(
            router,
            "/boot/manifest.ipxe?arch=arm64&ip=192.168.213.100&mac=&product=&manufacturer=",
        )
        .await;

        let seen = selector.0.lock().unwrap();
        assert_eq!(seen[0].arch, Arch::X86_64);
        assert_eq!(seen[0].ip, CLIENT_IP);
        assert_eq!(
            seen[0].mac.map(|mac| mac.to_string()).as_deref(),
            Some("52:54:00:ab:cd:ef")
        );
        assert_eq!(
            seen[0].product.as_deref(),
            Some("Standard PC (Q35 + ICH9, 2009)")
        );
        assert_eq!(seen[0].manufacturer.as_deref(), Some("Dell Inc.&Co"));
        assert_eq!(seen[1], BootClient::new(Arch::Arm64, CLIENT_IP));
    }
}

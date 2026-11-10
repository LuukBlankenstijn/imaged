use std::{
    collections::{BTreeMap, HashMap, hash_map::Entry},
    fmt, io,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError, RwLock},
};

use axum::body::Bytes;
use serde::Deserialize;

use crate::{
    pxe::Arch,
    select::{BootClient, ProductSelector, ProfileName, ProfileSelector},
};

struct BuiltIn {
    kernel: &'static [u8],
    initramfs: &'static [u8],
    modules: &'static [u8],
}

#[cfg(feature = "x86_64")]
static X86_64: BuiltIn = BuiltIn {
    kernel: include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/x86_64/vmlinuz")),
    initramfs: include_bytes!(concat!(
        env!("IMAGED_BOOT_ASSETS_DIR"),
        "/x86_64/initramfs.cpio.xz"
    )),
    modules: include_bytes!(concat!(
        env!("IMAGED_BOOT_ASSETS_DIR"),
        "/x86_64/modules.cpio.xz"
    )),
};

#[cfg(feature = "arm64")]
static ARM64: BuiltIn = BuiltIn {
    kernel: include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/arm64/vmlinuz")),
    initramfs: include_bytes!(concat!(
        env!("IMAGED_BOOT_ASSETS_DIR"),
        "/arm64/initramfs.cpio.xz"
    )),
    modules: include_bytes!(concat!(
        env!("IMAGED_BOOT_ASSETS_DIR"),
        "/arm64/modules.cpio.xz"
    )),
};

fn built_in(arch: Arch) -> Option<&'static BuiltIn> {
    match arch {
        #[cfg(feature = "x86_64")]
        Arch::X86_64 => Some(&X86_64),
        #[cfg(feature = "arm64")]
        Arch::Arm64 => Some(&ARM64),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

const ARCHES: [Arch; 2] = [Arch::X86_64, Arch::Arm64];

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Sources {
    pub profiles: BTreeMap<ProfileName, ProfileSources>,
    pub products: BTreeMap<String, ProfileName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProfileSources {
    pub arch: Arch,
    #[serde(default)]
    pub kernel: Option<PathBuf>,
    #[serde(default)]
    pub initrds: Vec<PathBuf>,
    #[serde(default)]
    pub console: Option<String>,
}

impl ProfileSources {
    pub fn new(arch: Arch) -> Self {
        Self {
            arch,
            kernel: None,
            initrds: Vec::new(),
            console: None,
        }
    }
}

pub struct BootFiles {
    profiles: HashMap<ProfileName, Profile>,
    selector: Arc<dyn ProfileSelector>,
}

pub(crate) struct Profile {
    pub(crate) arch: Arch,
    pub(crate) kernel: Bytes,
    pub(crate) initramfs: Vec<Bytes>,
    pub(crate) console: Option<String>,
}

impl BootFiles {
    pub async fn load(
        sources: &Sources,
        selector: Option<Arc<dyn ProfileSelector>>,
    ) -> io::Result<Self> {
        validate(sources, selector.is_some())?;
        let mut loader = Loader::default();
        let mut profiles = HashMap::with_capacity(sources.profiles.len() + 2);
        for (name, profile) in &sources.profiles {
            profiles.insert(name.clone(), loader.profile(name, profile).await?);
        }
        for arch in ARCHES.into_iter().filter(|&arch| built_in(arch).is_some()) {
            if let Entry::Vacant(entry) = profiles.entry(ProfileName::of(arch)) {
                let profile = loader
                    .profile(entry.key(), &ProfileSources::new(arch))
                    .await?;
                entry.insert(profile);
            }
        }
        let selector = selector.unwrap_or_else(|| {
            Arc::new(ProductSelector(
                sources
                    .products
                    .iter()
                    .map(|(product, name)| (product.clone(), name.clone()))
                    .collect(),
            ))
        });
        Ok(Self { profiles, selector })
    }

    pub(crate) fn select(&self, client: &BootClient) -> Option<(&ProfileName, &Profile)> {
        let fallback = || self.entry(&ProfileName::of(client.arch));
        let Some(name) = self.selector.profile(client) else {
            return fallback();
        };
        match self.entry(&name) {
            None => {
                tracing::warn!(profile = %name, ?client, "the selector chose an unknown boot profile; booting the arch default");
                fallback()
            }
            Some((_, profile)) if profile.arch != client.arch => {
                tracing::warn!(profile = %name, profile_arch = %profile.arch, ?client, "the selector chose a boot profile for another arch; booting the arch default");
                fallback()
            }
            Some(selected) => Some(selected),
        }
    }

    pub(crate) fn profile(&self, name: &str) -> Option<&Profile> {
        self.named(name).map(|(_, profile)| profile)
    }

    pub(crate) fn named(&self, name: &str) -> Option<(&ProfileName, &Profile)> {
        self.entry(&name.parse().ok()?)
    }

    fn entry(&self, name: &ProfileName) -> Option<(&ProfileName, &Profile)> {
        self.profiles.get_key_value(name)
    }
}

#[derive(Clone)]
pub struct SharedBootFiles(Arc<RwLock<Arc<BootFiles>>>);

impl SharedBootFiles {
    pub fn new(files: BootFiles) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(files))))
    }

    pub fn replace(&self, files: BootFiles) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(files);
    }

    pub(crate) fn current(&self) -> Arc<BootFiles> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn validate(sources: &Sources, external_selector: bool) -> io::Result<()> {
    let invalid = |message: String| Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    for arch in ARCHES {
        if let Some(profile) = sources.profiles.get(&ProfileName::of(arch))
            && profile.arch != arch
        {
            return invalid(format!(
                "boot profile \"{arch}\" replaces the {arch} default, so its arch must be \"{arch}\""
            ));
        }
    }
    if external_selector && !sources.products.is_empty() {
        return invalid(
            "[boot.products] is set, but the embedding program selects boot profiles itself"
                .to_string(),
        );
    }
    for (product, name) in &sources.products {
        if product.is_empty() || product.trim() != product {
            return invalid(format!(
                "[boot.products] key {product:?} is empty or has surrounding whitespace"
            ));
        }
        if !sources.profiles.contains_key(name) {
            return invalid(format!(
                "[boot.products] maps {product:?} to \"{name}\", which is not in [boot.profiles]"
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct Loader {
    read: HashMap<PathBuf, Bytes>,
}

impl Loader {
    async fn profile(
        &mut self,
        name: &ProfileName,
        sources: &ProfileSources,
    ) -> io::Result<Profile> {
        let built_in = built_in(sources.arch).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "boot profile \"{name}\" is for {}, but this imaged was built without the {} boot files",
                    sources.arch, sources.arch
                ),
            )
        })?;
        let mut initramfs = Vec::new();
        push_aligned(&mut initramfs, Bytes::from_static(built_in.initramfs));
        let kernel = match &sources.kernel {
            Some(path) => self.read(path).await?,
            None => {
                push_aligned(&mut initramfs, Bytes::from_static(built_in.modules));
                Bytes::from_static(built_in.kernel)
            }
        };
        for path in &sources.initrds {
            push_aligned(&mut initramfs, self.read(path).await?);
        }
        let profile = Profile {
            arch: sources.arch,
            kernel,
            initramfs,
            console: sources.console.clone(),
        };
        tracing::info!(
            profile = %name,
            arch = %sources.arch,
            kernel = %KernelSource(sources.kernel.as_deref()),
            initrds = ?sources.initrds,
            initramfs_size = profile.initramfs_size(),
            "boot profile loaded"
        );
        Ok(profile)
    }

    async fn read(&mut self, path: &Path) -> io::Result<Bytes> {
        if let Some(bytes) = self.read.get(path) {
            return Ok(bytes.clone());
        }
        let bytes = tokio::fs::read(path)
            .await
            .map(Bytes::from)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("boot file {}: {error}", path.display()),
                )
            })?;
        self.read.insert(path.to_path_buf(), bytes.clone());
        Ok(bytes)
    }
}

impl Profile {
    pub(crate) fn initramfs_size(&self) -> u64 {
        self.initramfs.iter().map(|part| part.len() as u64).sum()
    }
}

static PADDING: [u8; 3] = [0; 3];

fn push_aligned(initramfs: &mut Vec<Bytes>, part: Bytes) {
    let misalignment = part.len() % 4;
    initramfs.push(part);
    if misalignment != 0 {
        initramfs.push(Bytes::from_static(&PADDING[..4 - misalignment]));
    }
}

struct KernelSource<'a>(Option<&'a Path>);

impl fmt::Display for KernelSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(path) => path.display().fmt(f),
            None => f.write_str("built-in, with its modules"),
        }
    }
}

#[cfg(all(test, feature = "x86_64", feature = "arm64"))]
pub(crate) mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    pub(crate) struct TempFile(pub(crate) PathBuf);

    impl TempFile {
        pub(crate) fn new(name: &str, contents: &[u8]) -> Self {
            let path =
                std::env::temp_dir().join(format!("imaged-boot-{}-{name}", std::process::id()));
            std::fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    pub(crate) const CLIENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 213, 100));

    pub(crate) fn name(name: &str) -> ProfileName {
        name.parse().unwrap()
    }

    pub(crate) fn profile_with(
        arch: Arch,
        kernel: Option<&TempFile>,
        initrds: &[&TempFile],
    ) -> ProfileSources {
        let mut profile = ProfileSources::new(arch);
        profile.kernel = kernel.map(|file| file.0.clone());
        profile.initrds = initrds.iter().map(|file| file.0.clone()).collect();
        profile
    }

    #[derive(Debug)]
    pub(crate) struct Fixed(pub(crate) Option<&'static str>);

    impl ProfileSelector for Fixed {
        fn profile(&self, _: &BootClient) -> Option<ProfileName> {
            self.0.map(name)
        }
    }

    fn served(profile: &Profile) -> Vec<u8> {
        profile.initramfs.concat()
    }

    fn padded(part: &[u8]) -> Vec<u8> {
        let mut padded = part.to_vec();
        padded.resize(part.len().next_multiple_of(4), 0);
        padded
    }

    fn client(arch: Arch, product: Option<&str>) -> BootClient {
        let mut client = BootClient::new(arch, CLIENT_IP);
        client.product = product.map(str::to_string);
        client
    }

    async fn load(sources: &Sources) -> BootFiles {
        BootFiles::load(sources, None).await.unwrap()
    }

    async fn load_error(sources: &Sources, selector: Option<Arc<dyn ProfileSelector>>) -> String {
        let error = BootFiles::load(sources, selector).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        error.to_string()
    }

    #[tokio::test]
    async fn the_arch_profiles_are_built_in_with_the_built_in_kernels_modules() {
        let files = load(&Sources::default()).await;

        let x86_64 = files.profile("x86_64").unwrap();
        assert_eq!(x86_64.kernel, X86_64.kernel);
        assert_eq!(
            served(x86_64),
            [padded(X86_64.initramfs), padded(X86_64.modules)].concat()
        );
        let arm64 = files.profile("arm64").unwrap();
        assert_eq!(arm64.kernel, ARM64.kernel);
        assert_eq!(
            served(arm64),
            [padded(ARM64.initramfs), padded(ARM64.modules)].concat()
        );
    }

    #[tokio::test]
    async fn initrds_follow_the_built_in_ones_in_order_each_starting_4_byte_aligned() {
        let first = TempFile::new("extra-first", b"first");
        let second = TempFile::new("extra-second", b"second");
        let mut sources = Sources::default();
        sources.profiles.insert(
            name("arm64"),
            profile_with(Arch::Arm64, None, &[&first, &second]),
        );

        let files = load(&sources).await;

        let arm64 = files.profile("arm64").unwrap();
        let built_in = [padded(ARM64.initramfs), padded(ARM64.modules)].concat();
        let served_arm64 = served(arm64);
        assert_eq!(&served_arm64[..built_in.len()], built_in);
        assert_eq!(&served_arm64[built_in.len()..], b"first\0\0\0second\0\0");
        assert_eq!(arm64.initramfs_size(), served_arm64.len() as u64);
        assert_eq!(
            served(files.profile("x86_64").unwrap()),
            [padded(X86_64.initramfs), padded(X86_64.modules)].concat()
        );
    }

    #[tokio::test]
    async fn a_profile_with_its_own_kernel_leaves_out_the_built_in_modules() {
        let kernel = TempFile::new("profile-kernel", b"jetson");
        let modules = TempFile::new("profile-modules", b"tegra");
        let mut sources = Sources::default();
        sources.profiles.insert(
            name("jetson"),
            profile_with(Arch::Arm64, Some(&kernel), &[&modules]),
        );

        let files = load(&sources).await;

        let jetson = files.profile("jetson").unwrap();
        assert_eq!(jetson.kernel, &b"jetson"[..]);
        assert_eq!(
            served(jetson),
            [padded(ARM64.initramfs), padded(b"tegra")].concat()
        );
        assert!(files.profile("unknown").is_none());
        assert!(files.profile("../jetson").is_none());
    }

    #[tokio::test]
    async fn a_missing_boot_file_fails_naming_it() {
        let missing = std::env::temp_dir().join("imaged-boot-missing-initrd.cpio.xz");
        let mut profile = ProfileSources::new(Arch::X86_64);
        profile.initrds = vec![missing.clone()];
        let mut sources = Sources::default();
        sources.profiles.insert(name("x86_64"), profile);

        let error = BootFiles::load(&sources, None).await.err().unwrap();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains(&missing.display().to_string()),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_selectors_answer_counts_only_for_a_known_profile_of_the_clients_arch() {
        let mut sources = Sources::default();
        let mut jetson = ProfileSources::new(Arch::Arm64);
        jetson.console = Some("ttyTCU0,115200n8".to_string());
        sources.profiles.insert(name("jetson"), jetson);

        for (answer, arch, expected) in [
            (Some("jetson"), Arch::Arm64, "jetson"),
            (Some("jetson"), Arch::X86_64, "x86_64"),
            (Some("unknown"), Arch::Arm64, "arm64"),
            (Some("x86_64"), Arch::Arm64, "arm64"),
            (None, Arch::Arm64, "arm64"),
            (None, Arch::X86_64, "x86_64"),
        ] {
            let files = BootFiles::load(&sources, Some(Arc::new(Fixed(answer))))
                .await
                .unwrap();
            let (selected, profile) = files.select(&client(arch, None)).unwrap();
            assert_eq!(selected.as_str(), expected, "{answer:?} on {arch}");
            assert_eq!(profile.arch, arch);
            assert_eq!(
                profile.console.as_deref(),
                (expected == "jetson").then_some("ttyTCU0,115200n8")
            );
        }
    }

    #[tokio::test]
    async fn the_built_in_selector_matches_the_reported_product_exactly() {
        let mut sources = Sources::default();
        sources
            .profiles
            .insert(name("jetson"), ProfileSources::new(Arch::Arm64));
        sources.products.insert(
            "NVIDIA Jetson Orin Nano Developer Kit".to_string(),
            name("jetson"),
        );
        let files = load(&sources).await;

        for (product, expected) in [
            (Some("NVIDIA Jetson Orin Nano Developer Kit"), "jetson"),
            (Some(" NVIDIA Jetson Orin Nano Developer Kit "), "jetson"),
            (Some("NVIDIA Jetson Orin Nano"), "arm64"),
            (Some("nvidia jetson orin nano developer kit"), "arm64"),
            (None, "arm64"),
        ] {
            assert_eq!(
                files
                    .select(&client(Arch::Arm64, product))
                    .unwrap()
                    .0
                    .as_str(),
                expected,
                "{product:?}"
            );
        }
    }

    #[tokio::test]
    async fn inconsistent_boot_tables_are_rejected_before_any_file_is_read() {
        let mut wrong_arch = Sources::default();
        wrong_arch
            .profiles
            .insert(name("arm64"), ProfileSources::new(Arch::X86_64));
        assert!(
            load_error(&wrong_arch, None)
                .await
                .contains("\"arm64\" replaces the arm64 default, so its arch must be \"arm64\"")
        );

        let mut unknown = Sources::default();
        unknown.products.insert("Box".to_string(), name("missing"));
        assert!(
            load_error(&unknown, None)
                .await
                .contains("\"Box\" to \"missing\", which is not in [boot.profiles]")
        );

        let mut padded_key = Sources::default();
        padded_key
            .profiles
            .insert(name("box"), ProfileSources::new(Arch::X86_64));
        padded_key.products.insert("Box ".to_string(), name("box"));
        assert!(
            load_error(&padded_key, None)
                .await
                .contains("surrounding whitespace")
        );

        let mut both = Sources::default();
        both.profiles
            .insert(name("box"), ProfileSources::new(Arch::X86_64));
        both.products.insert("Box".to_string(), name("box"));
        assert!(
            load_error(&both, Some(Arc::new(Fixed(None))))
                .await
                .contains("selects boot profiles itself")
        );
    }

    #[test]
    fn profile_names_are_url_safe() {
        for valid in ["jetson-orin-nano", "orange_pi_5", "x86_64", "A1"] {
            assert!(valid.parse::<ProfileName>().is_ok(), "{valid}");
        }
        for invalid in ["", "a b", "../x", "x/y", "é"] {
            assert!(invalid.parse::<ProfileName>().is_err(), "{invalid}");
        }
    }
}

#[cfg(all(test, not(feature = "x86_64")))]
mod arm64_only_tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[tokio::test]
    async fn an_arch_left_out_of_the_build_has_no_profiles() {
        let files = BootFiles::load(&Sources::default(), None).await.unwrap();
        let client = BootClient::new(Arch::X86_64, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(files.select(&client).is_none());
        assert!(files.profile("x86_64").is_none());
        assert!(files.profile("arm64").is_some());

        let mut sources = Sources::default();
        sources
            .profiles
            .insert("box".parse().unwrap(), ProfileSources::new(Arch::X86_64));
        let error = BootFiles::load(&sources, None).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error
                .to_string()
                .contains("built without the x86_64 boot files"),
            "{error}"
        );
    }
}

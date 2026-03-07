#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

#[cfg(feature = "backend")]
pub use molt_lang_backend as backend;
#[cfg(feature = "db")]
pub use molt_lang_db as db;
#[cfg(feature = "obj-model")]
pub use molt_lang_obj_model as obj_model;
#[cfg(feature = "runtime")]
pub use molt_lang_runtime as runtime;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const HOMEPAGE: &str = match option_env!("CARGO_PKG_HOMEPAGE") {
    Some(value) => value,
    None => "https://github.com/adpena/molt",
};
pub const REPOSITORY: &str = match option_env!("CARGO_PKG_REPOSITORY") {
    Some(value) => value,
    None => "https://github.com/adpena/molt",
};
pub const DOCUMENTATION: &str = match option_env!("CARGO_PKG_DOCUMENTATION") {
    Some(value) => value,
    None => "https://docs.rs/molt-lang-python",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistributionMetadata {
    pub crate_name: &'static str,
    pub version: &'static str,
    pub homepage: &'static str,
    pub repository: &'static str,
    pub documentation: &'static str,
}

pub const fn distribution_metadata() -> DistributionMetadata {
    DistributionMetadata {
        crate_name: CRATE_NAME,
        version: VERSION,
        homepage: HOMEPAGE,
        repository: REPOSITORY,
        documentation: DOCUMENTATION,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Molt,
    MoltWorker,
}

impl ArtifactKind {
    const fn asset_stem(self) -> &'static str {
        match self {
            Self::Molt => "molt",
            Self::MoltWorker => "molt-worker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    const fn asset_segment(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Macos | Self::Linux => "tar.gz",
            Self::Windows => "zip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Arm64,
    Aarch64,
}

impl Arch {
    const fn asset_segment(self, platform: Platform) -> Option<&'static str> {
        match (platform, self) {
            (Platform::Macos, Self::Arm64) => Some("arm64"),
            (Platform::Macos, Self::X86_64) => Some("x86_64"),
            (Platform::Linux, Self::Aarch64) => Some("aarch64"),
            (Platform::Linux, Self::X86_64) => Some("x86_64"),
            (Platform::Windows, Self::X86_64) => Some("x86_64"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseArtifactSpec {
    pub kind: ArtifactKind,
    pub platform: Platform,
    pub arch: Arch,
}

impl ReleaseArtifactSpec {
    pub const fn new(kind: ArtifactKind, platform: Platform, arch: Arch) -> Self {
        Self {
            kind,
            platform,
            arch,
        }
    }

    pub const fn is_supported(self) -> bool {
        self.arch.asset_segment(self.platform).is_some()
    }
}

pub fn release_asset_name(version: &str, spec: ReleaseArtifactSpec) -> Option<String> {
    let arch = spec.arch.asset_segment(spec.platform)?;
    let version = normalize_version(version);
    Some(format!(
        "{}-{}-{}-{}.{}",
        spec.kind.asset_stem(),
        version,
        spec.platform.asset_segment(),
        arch,
        spec.platform.extension(),
    ))
}

pub fn github_release_url(
    owner: &str,
    repo: &str,
    version: &str,
    spec: ReleaseArtifactSpec,
) -> Option<String> {
    let asset = release_asset_name(version, spec)?;
    let version = normalize_version(version);
    Some(format!(
        "https://github.com/{owner}/{repo}/releases/download/v{version}/{asset}"
    ))
}

fn normalize_version(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::{
        Arch, ArtifactKind, CRATE_NAME, Platform, ReleaseArtifactSpec, VERSION,
        distribution_metadata, github_release_url, release_asset_name,
    };

    #[test]
    fn metadata_matches_package_manifest() {
        let metadata = distribution_metadata();
        assert_eq!(metadata.crate_name, CRATE_NAME);
        assert_eq!(metadata.version, VERSION);
        assert_eq!(metadata.homepage, "https://github.com/adpena/molt");
        assert_eq!(metadata.repository, "https://github.com/adpena/molt");
        assert_eq!(metadata.documentation, "https://docs.rs/molt-lang-python");
    }

    #[test]
    fn release_asset_name_matches_release_workflow_convention() {
        let name = release_asset_name(
            "v0.0.1",
            ReleaseArtifactSpec::new(ArtifactKind::Molt, Platform::Macos, Arch::Arm64),
        );
        assert_eq!(name.as_deref(), Some("molt-0.0.1-macos-arm64.tar.gz"));
    }

    #[test]
    fn worker_windows_artifacts_use_zip_bundles() {
        let name = release_asset_name(
            "0.0.1",
            ReleaseArtifactSpec::new(ArtifactKind::MoltWorker, Platform::Windows, Arch::X86_64),
        );
        assert_eq!(
            name.as_deref(),
            Some("molt-worker-0.0.1-windows-x86_64.zip")
        );
    }

    #[test]
    fn unsupported_target_triplets_fail_closed() {
        let unsupported =
            ReleaseArtifactSpec::new(ArtifactKind::Molt, Platform::Windows, Arch::Arm64);
        assert!(!unsupported.is_supported());
        assert_eq!(release_asset_name("0.0.1", unsupported), None);
        assert_eq!(
            github_release_url("adpena", "molt", "0.0.1", unsupported),
            None
        );
    }

    #[test]
    fn github_release_url_tracks_canonical_tag_shape() {
        let url = github_release_url(
            "adpena",
            "molt",
            "0.0.1",
            ReleaseArtifactSpec::new(ArtifactKind::Molt, Platform::Linux, Arch::Aarch64),
        );
        assert_eq!(
            url.as_deref(),
            Some(
                "https://github.com/adpena/molt/releases/download/v0.0.1/molt-0.0.1-linux-aarch64.tar.gz",
            ),
        );
    }
}

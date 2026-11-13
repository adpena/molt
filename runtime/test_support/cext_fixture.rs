// Shared native C-extension fixture builder for runtime-backed ABI tests.
// Extensions link against the one-image `cext_host` example Cargo built for the
// running test image's exact target/profile, named by MOLT_CEXT_HOST_LIBRARY;
// CargoTestArtifacts owns every generated input and output. No standalone ABI
// library, ambient target/profile probing or temporary-directory reuse is
// permitted.

#[cfg(windows)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::cargo_test_artifacts::CargoTestArtifacts;

/// An explicitly built one-image host example for the running test image.
pub struct HostArtifact {
    /// The exact image that fixture extensions bind their imports to.
    pub library: PathBuf,
    #[cfg(not(windows))]
    examples_dir: PathBuf,
    #[cfg(not(windows))]
    link_name: String,
    #[cfg(windows)]
    import_library: PathBuf,
}

impl HostArtifact {
    pub fn for_current_test_image(example: &str) -> Self {
        // Cargo writes this test image to its build directory, whose layout
        // is not a stable interface, so the image cannot find the example
        // from its own path. The caller builds the example in this test's
        // target/profile and names the image Cargo's JSON `filenames` report.
        let build = format!(
            "cargo build -p molt-runtime --example {example} --features cext_loader \
             --message-format=json (with this test's --target, profile and features), then \
             set MOLT_CEXT_HOST_LIBRARY to the reported {example} cdylib"
        );
        let library = std::env::var_os("MOLT_CEXT_HOST_LIBRARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("MOLT_CEXT_HOST_LIBRARY is unset: {build}"));
        // These paths reach compiler and linker arguments. Canonicalization
        // would turn them into Windows verbatim paths, which tools misparse.
        let library = std::path::absolute(&library)
            .unwrap_or_else(|error| panic!("locate {library:?}: {error}"));
        assert!(
            library.is_file(),
            "one-image C-extension host is absent: {library:?}; {build}"
        );
        let examples_dir = library
            .parent()
            .expect("C-extension host image parent")
            .to_path_buf();

        #[cfg(windows)]
        let (library, import_library) = {
            let import_library = examples_dir.join(format!("{example}.dll.lib"));
            assert!(
                import_library.is_file(),
                "host import library is absent: {import_library:?}; rebuild it: {build}"
            );
            // Windows binds imports by the DLL name the import library
            // records, not by the path a caller loaded. Cargo may link the
            // example under a metadata-suffixed name and uplift a copy, so load
            // exactly the recorded image.
            let library = examples_dir.join(import_library_dll_name(&import_library));
            assert!(
                library.is_file(),
                "host DLL named by {import_library:?} is absent: {library:?}; rebuild it: {build}"
            );
            (library, import_library)
        };

        Self {
            library,
            #[cfg(not(windows))]
            examples_dir,
            #[cfg(not(windows))]
            link_name: example.to_owned(),
            #[cfg(windows)]
            import_library,
        }
    }
}

/// Read the single DLL name recorded by the short import objects of a
/// Windows import library archive.
#[cfg(windows)]
fn import_library_dll_name(library: &Path) -> OsString {
    let bytes = std::fs::read(library)
        .unwrap_or_else(|error| panic!("read import library {library:?}: {error}"));
    assert!(
        bytes.starts_with(b"!<arch>\n"),
        "import library is not an archive: {library:?}"
    );
    let mut names = std::collections::BTreeSet::new();
    let mut offset = 8;
    while offset + 60 <= bytes.len() {
        let size: usize = std::str::from_utf8(&bytes[offset + 48..offset + 58])
            .ok()
            .and_then(|field| field.trim().parse().ok())
            .unwrap_or_else(|| panic!("malformed archive member header in {library:?}"));
        let start = offset + 60;
        let member = bytes
            .get(start..start + size)
            .unwrap_or_else(|| panic!("truncated archive member in {library:?}"));
        // IMPORT_OBJECT_HEADER: Sig1 = 0, Sig2 = 0xFFFF, Version = 0; the
        // symbol and DLL names follow its 20 bytes, each NUL-terminated.
        if member.len() >= 20 && member[..6] == [0, 0, 0xff, 0xff, 0, 0] {
            let data_len = u32::from_le_bytes(member[12..16].try_into().unwrap()) as usize;
            let mut strings = member[20..]
                .get(..data_len)
                .unwrap_or_else(|| panic!("truncated import object in {library:?}"))
                .split(|byte| *byte == 0);
            let _symbol = strings.next();
            if let Some(dll) = strings.next().filter(|dll| !dll.is_empty()) {
                names.insert(dll.to_vec());
            }
        }
        offset = start + size + (size & 1);
    }
    assert_eq!(
        names.len(),
        1,
        "import library must name exactly one DLL: {library:?}"
    );
    let name = names.into_iter().next().unwrap();
    OsString::from(String::from_utf8(name).expect("import library DLL name is UTF-8"))
}

pub fn build_extension(
    artifacts: &CargoTestArtifacts,
    host: &HostArtifact,
    source: &Path,
    module: &str,
) -> PathBuf {
    assert!(
        !module.is_empty()
            && module
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "fixture module must have a portable C identifier"
    );
    assert!(
        source.is_file(),
        "C-extension fixture is absent: {source:?}"
    );
    let source_copy = artifacts.path().join(format!("{module}.c"));
    std::fs::copy(source, &source_copy).expect("copy C fixture into Cargo artifact custody");
    let output = artifacts.path().join(format!(
        "{module}.{}",
        if cfg!(windows) { "pyd" } else { "so" }
    ));
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let abi_include = manifest.join("../molt-cpython-abi/include");
    let shared_include = manifest.join("../../include/molt/shared");
    let target = env!("MOLT_CARGO_TARGET");
    let mut compiler = cc::Build::new();
    // Outside a build script cc has no OPT_LEVEL/DEBUG environment; fix both.
    compiler
        .target(target)
        .host(env!("MOLT_CARGO_HOST"))
        .opt_level(0)
        .debug(false)
        .cargo_metadata(false)
        .warnings(false);
    let tool = compiler
        .try_get_compiler()
        .unwrap_or_else(|error| panic!("discover C compiler for {target}: {error}"));
    let mut command = tool.to_command();
    command.current_dir(artifacts.path());

    #[cfg(windows)]
    {
        assert!(
            tool.is_like_msvc(),
            "Windows host fixture must use an MSVC-compatible compiler: {tool:?}"
        );
        command
            .arg("/LD")
            .arg("/DMOLT_CPYTHON_ABI_SHARED=1")
            .arg(format!("/I{}", abi_include.display()))
            .arg(format!("/I{}", shared_include.display()))
            .arg(artifacts.argument("", &source_copy).expect("owned C input"))
            .arg("/link")
            .arg(
                artifacts
                    .argument("/OUT:", &output)
                    .expect("owned extension output"),
            )
            .arg(&host.import_library);
    }
    #[cfg(not(windows))]
    {
        command
            .arg(if cfg!(target_os = "macos") {
                "-dynamiclib"
            } else {
                "-shared"
            })
            .arg("-fPIC")
            .arg("-fvisibility=hidden")
            .arg("-DMOLT_CPYTHON_ABI_SHARED=1")
            .arg("-I")
            .arg(&abi_include)
            .arg("-I")
            .arg(&shared_include)
            .arg(artifacts.argument("", &source_copy).expect("owned C input"))
            .arg("-L")
            .arg(&host.examples_dir)
            .arg(format!("-l{}", host.link_name))
            .arg(format!("-Wl,-rpath,{}", host.examples_dir.display()))
            .arg("-o")
            .arg(
                artifacts
                    .argument("", &output)
                    .expect("owned extension output"),
            );
    }

    let result = command
        .output()
        .unwrap_or_else(|error| panic!("compile {module} C extension: {error}"));
    assert!(
        result.status.success(),
        "C extension fixture compile failed: status={}\nstdout:\n{}\nstderr:\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output.is_file(),
        "compiled C extension is absent: {output:?}"
    );
    output
}

//! Build-script projection of the selected SDK C ABI.
//!
//! Wire owner: molt.wasi_sdk_identity.WasiCAbiProjection. This decoder checks
//! the bounded transport, target/variant, paths and extents. SHA256 fields are
//! content declarations checked by Python command custody, not authenticated
//! evidence merely because they arrived in an environment variable.
#[cfg(not(test))]
use std::env;
use std::path::{Component, Path, PathBuf};

pub const PLAN_ENV: &str = "MOLT_WASI_C_ABI_PLAN";
const PROTOCOL: &str = include_str!("../../src/molt/wasi_c_abi_protocol.txt");
fn declaration(name: &str) -> &'static str {
    let mut values = PROTOCOL.lines().filter_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key == name).then_some(value)
    });
    let value = values
        .next()
        .expect("missing canonical WASI protocol declaration");
    assert!(
        values.next().is_none(),
        "duplicate canonical WASI protocol declaration"
    );
    value
}
fn bound(name: &str) -> usize {
    declaration(name)
        .parse()
        .expect("invalid canonical WASI protocol bound")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WasiCAbiPlan {
    pub sdk: PathBuf,
    pub root: PathBuf,
    pub include_dir: PathBuf,
    pub driver: PathBuf,
    pub linker: PathBuf,
    pub members: Vec<(String, PathBuf)>,
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn version(value: &str, components: usize) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == components && parts.iter().all(|part| decimal(part))
}

fn sdk_version(value: &str) -> bool {
    let (base, suffix) = value
        .split_once('+')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    version(base, 2)
        && suffix.is_none_or(|s| {
            !s.is_empty()
                && s.as_bytes()[0].is_ascii_alphanumeric()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

fn absolute_path(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if raw.chars().count() > bound("max_path_chars")
        || !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err("WASI C-runtime path is not canonical absolute syntax".into());
    }
    Ok(path)
}

// rustc 1.99 --help -v outer arity, mirrored by molt.rust_toolchain.
// A short value-taking option consumes the remainder of its cluster or the
// next raw token. Preceding h/V/v/g/O switches do not change that boundary.
fn rust_argument_value<'a>(
    argument: &'a str,
    arguments: &mut impl Iterator<Item = &'a str>,
) -> Result<Option<(&'a str, &'a str)>, String> {
    let mut selected = None;
    let mut operand = None;
    if argument.starts_with("--") {
        let (key, value) = argument
            .split_once('=')
            .map_or((argument, None), |(k, v)| (k, Some(v)));
        if key == "--codegen" {
            selected = Some("-C");
            operand = value;
        } else if matches!(
            key,
            "--cfg"
                | "--check-cfg"
                | "--crate-type"
                | "--crate-name"
                | "--edition"
                | "--emit"
                | "--print"
                | "--sysroot"
                | "--target"
                | "--extern"
                | "--out-dir"
                | "--explain"
                | "--color"
                | "--error-format"
                | "--json"
                | "--diagnostic-width"
                | "--remap-path-prefix"
                | "--remap-path-scope"
                | "--cap-lints"
                | "--force-warn"
                | "--allow"
                | "--warn"
                | "--deny"
                | "--forbid"
        ) {
            selected = Some(key);
            operand = value;
        }
    } else if let Some(short) = argument.strip_prefix('-') {
        for (offset, character) in short.char_indices() {
            if matches!(character, 'h' | 'V' | 'v' | 'g' | 'O') {
                continue;
            }
            selected = match character {
                'C' => Some("-C"),
                'L' => Some("-L"),
                'l' => Some("-l"),
                'o' => Some("-o"),
                'A' => Some("-A"),
                'W' => Some("-W"),
                'D' => Some("-D"),
                'F' => Some("-F"),
                'Z' => Some("-Z"),
                _ => None,
            };
            let rest = &short[offset + character.len_utf8()..];
            if !rest.is_empty() {
                operand = Some(rest);
            }
            break;
        }
    }
    let Some(key) = selected else {
        return Ok(None);
    };
    let value = operand
        .or_else(|| arguments.next())
        .ok_or("incomplete WASI Rust operand")?;
    Ok(Some((key, value)))
}

impl WasiCAbiPlan {
    pub fn decode(value: &str) -> Result<Self, String> {
        let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
        if value.is_empty()
            || value.len() > bound("max_chars")
            || !remainder.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid bounded WASI C-runtime projection encoding".into());
        }
        let nibble = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        let bytes: Vec<u8> = pairs
            .iter()
            .map(|pair| (nibble(pair[0]) << 4) | nibble(pair[1]))
            .collect();
        let text =
            std::str::from_utf8(&bytes).map_err(|_| "WASI C-runtime projection is not UTF-8")?;
        let fields: Vec<_> = text.split('\0').collect();
        let header: Vec<_> = declaration("header").split(',').collect();
        let roles: Vec<_> = declaration("members").split(',').collect();
        let start = 3 + header.len();
        if fields.len() != start + 4 * roles.len()
            || fields[..3]
                != [
                    declaration("schema"),
                    declaration("target"),
                    declaration("variant"),
                ]
        {
            return Err("invalid WASI C-runtime schema/target/variant".into());
        }
        let field = |name: &str| {
            fields[3 + header
                .iter()
                .position(|key| *key == name)
                .expect("canonical WASI protocol header")]
        };
        if field("sdk_version").len() > bound("max_version_chars")
            || field("llvm_version").len() > bound("max_version_chars")
            || !sdk_version(field("sdk_version"))
            || !version(field("llvm_version"), 3)
            || !sha256(field("tree_sha256"))
        {
            return Err("invalid WASI C-runtime producer".into());
        }
        let sdk = absolute_path(field("sdk"))?;
        let root = absolute_path(field("sysroot"))?;
        let include_dir = absolute_path(field("include"))?;
        let driver = absolute_path(field("driver"))?;
        let linker = absolute_path(field("linker"))?;
        if !root.starts_with(&sdk)
            || !include_dir.starts_with(&root)
            || !driver.starts_with(&sdk)
            || !linker.starts_with(&sdk)
        {
            return Err("WASI C-runtime roots escape their SDK".into());
        }
        // Compare native canonical paths to each other; Windows canonicalization
        // may add the extended-length prefix. Wire syntax remains untouched.
        let physical_sdk = std::fs::canonicalize(&sdk).map_err(|e| format!("WASI SDK: {e}"))?;
        let mut members = Vec::with_capacity(roles.len());
        for (index, role) in roles.iter().enumerate() {
            let base = start + 4 * index;
            let size = fields[base + 2];
            if fields[base] != *role
                || !sha256(fields[base + 3])
                || !decimal(size)
                || (size.len() > 1 && size.starts_with('0'))
                || size.len() > 20
            {
                return Err(format!("invalid WASI C-runtime member: {role}"));
            }
            let size = size
                .parse::<u64>()
                .map_err(|_| "WASI member extent overflows")?;
            let max: u64 = declaration("max_member_bytes")
                .parse()
                .expect("canonical member extent");
            if size > max {
                return Err("WASI member extent exceeds bound".into());
            }
            let path = absolute_path(fields[base + 1])?;
            if !path.starts_with(&sdk) {
                return Err(format!("WASI member escapes SDK: {role}"));
            }
            let actual = std::fs::canonicalize(&path).map_err(|e| format!("WASI {role}: {e}"))?;
            if !actual.starts_with(&physical_sdk) {
                return Err(format!("WASI C-runtime member redirects: {role}"));
            }
            let metadata = std::fs::metadata(&path).map_err(|e| format!("WASI {role}: {e}"))?;
            if !metadata.is_file() || metadata.len() != size {
                return Err(format!("WASI C-runtime member extent changed: {role}"));
            }
            members.push(((*role).to_owned(), path));
        }
        Ok(Self {
            sdk,
            root,
            include_dir,
            driver,
            linker,
            members,
        })
    }

    pub fn native_search_directories(&self) -> Vec<&Path> {
        let mut paths = Vec::new();
        for (_, member) in &self.members {
            let path = member.parent().expect("SDK member parent");
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    pub fn validate_cargo_mode(
        &self,
        target: &str,
        linker: &str,
        flags: &str,
    ) -> Result<(), String> {
        if (target != "wasm32-unknown-unknown" && target != declaration("target"))
            || Path::new(linker) != self.linker
        {
            return Err("WASI Cargo target/linker differs from the selected SDK raw linker".into());
        }
        // Cargo documents these as the effective target linker and encoded
        // dependency flags. Final cargo-rustc arguments have separate parent
        // command custody; this is not a receipt for arbitrary final overrides.
        let mut arguments = flags.split('\x1f');
        let mut external = false;
        let mut raw_linker = false;
        let mut search = Vec::new();
        while let Some(argument) = arguments.next() {
            if argument == "--" {
                break;
            }
            let Some((key, value)) = rust_argument_value(argument, &mut arguments)? else {
                continue;
            };
            if key == "-L" {
                let (kind, path) = value.split_once('=').unwrap_or(("all", value));
                let (kind, path) = if matches!(
                    kind,
                    "dependency" | "crate" | "native" | "framework" | "all"
                ) {
                    (kind, path)
                } else {
                    ("all", value)
                };
                if kind == "native" || kind == "all" {
                    search.push((kind, Path::new(path)));
                }
                continue;
            }
            if key != "-C" {
                continue;
            }
            if value.is_empty() || value.starts_with('=') || value.starts_with('-') {
                return Err("invalid WASI codegen option".into());
            }
            // Normalize only the option name. Paths, linker arguments and
            // feature values retain their exact bytes after the first '='.
            let (key, operand) = value.split_once('=').unwrap_or((value, ""));
            let key = key.replace('_', "-");
            if key == "link-self-contained" {
                if external || operand != declaration("link_self_contained") {
                    return Err("WASI requires external libc".into());
                }
                external = true;
            } else if key == "linker-flavor" {
                if raw_linker || operand != declaration("linker_flavor") {
                    return Err("WASI requires the stable raw wasm-ld flavor".into());
                }
                raw_linker = true;
            } else if key == "linker" && Path::new(operand) != self.linker {
                return Err("WASI Rust flag overrides selected raw linker".into());
            }
        }
        if !external || !raw_linker {
            return Err(format!(
                "WASI Cargo flags require link-self-contained={}, linker-flavor={}; use the project toolchain projection",
                declaration("link_self_contained"),
                declaration("linker_flavor")
            ));
        }
        let expected: Vec<_> = self
            .native_search_directories()
            .into_iter()
            .map(|path| ("native", path))
            .collect();
        if !search.starts_with(&expected) {
            return Err("WASI target flags must search the selected SDK directories first; use the project toolchain projection".into());
        }
        Ok(())
    }

    // Build scripts admit the Cargo environment; wire tests admit explicit values.
    #[cfg(not(test))]
    pub fn from_environment() -> Self {
        println!("cargo:rerun-if-env-changed={PLAN_ENV}");
        let raw = env::var(PLAN_ENV).unwrap_or_else(|_| panic!(
            "WASI C-runtime plan missing: provision the pinned SDK and project the project WASM toolchain environment before Cargo"
        ));
        let plan =
            Self::decode(&raw).unwrap_or_else(|e| panic!("WASI C-runtime plan refused: {e}"));
        for name in ["TARGET", "RUSTC_LINKER", "CARGO_ENCODED_RUSTFLAGS"] {
            println!("cargo:rerun-if-env-changed={name}");
        }
        plan.validate_cargo_mode(
            &env::var("TARGET").expect("Cargo TARGET"),
            &env::var("RUSTC_LINKER").unwrap_or_default(),
            &env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default(),
        )
        .unwrap_or_else(|e| panic!("WASI Cargo mode refused: {e}"));
        for (_role, path) in &plan.members {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        println!("cargo:rerun-if-changed={}", plan.include_dir.display());
        plan
    }
}

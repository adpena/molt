use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cc::Build;

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_ptr_width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    // Keep cdylib in the crate types so plain `cargo build -p molt-runtime`
    // still emits a stable `molt_runtime.wasm` artifact for wasm lanes that
    // consume the runtime directly.
    let _ = &target_os;
    println!("cargo:rustc-check-cfg=cfg(molt_has_mpdec)");

    // Emit `molt_has_net_io` when both non-WASM *and* stdlib_net feature are
    // active.  This replaces hundreds of bare `cfg(not(target_arch = "wasm32"))`
    // guards in the async-rt networking code so that omitting stdlib_net on
    // native targets lets the linker drop mio/rustls/tungstenite/socket2.
    println!("cargo:rustc-check-cfg=cfg(molt_has_net_io)");
    if target_arch != "wasm32" {
        // CARGO_FEATURE_<NAME> is set for every enabled Cargo feature.
        if env::var("CARGO_FEATURE_STDLIB_NET").is_ok() {
            println!("cargo:rustc-cfg=molt_has_net_io");
        }
    }

    if build_libmpdec(
        &manifest_dir,
        &out_dir,
        &target_env,
        &target_ptr_width,
        &target_arch,
    ) {
        println!("cargo:rustc-cfg=molt_has_mpdec");
    }

    emit_native_cdylib_isolate_stubs(&out_dir, &target_arch, &target_env);

    if target_arch != "wasm32" {
        let output = Command::new("python3")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(mut stdin) = child.stdin.take() {
                    let script = r#"
import errno
names = []
for name in dir(errno):
    if not name.startswith("E"):
        continue
    if not name[1:].isupper():
        continue
    val = getattr(errno, name)
    if isinstance(val, int):
        names.append((name, val))
for name, val in sorted(set(names)):
    print(f"{name},{val}")
"#;
                    stdin.write_all(script.as_bytes())?;
                }
                child.wait_with_output()
            });
        let output = match output {
            Ok(out) => out,
            Err(err) => {
                panic!("failed to run python3 to generate errno constants: {err}");
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("python3 errno generation failed: {stderr}");
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries: Vec<(String, i64)> = Vec::new();
        for line in stdout.lines() {
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            let Some((name, value)) = name.split_once(',') else {
                continue;
            };
            let value: i64 = match value.parse() {
                Ok(val) => val,
                Err(_) => continue,
            };
            entries.push((name.to_string(), value));
        }
        if entries.is_empty() {
            panic!("python3 errno generation returned no entries");
        }
        let mut out = String::new();
        out.push_str("pub(crate) fn collect_errno_constants() -> Vec<(&'static str, i64)> {\n");
        out.push_str("    vec![\n");
        for (name, value) in entries {
            out.push_str(&format!("        (\"{name}\", {value}i64),\n"));
        }
        out.push_str("    ]\n");
        out.push_str("}\n");
        fs::write(out_dir.join("errno_constants.rs"), out)
            .expect("failed to write errno_constants.rs");
    }

    let output = Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let script = r#"
import unicodedata

KEYS = ("digit", "decimal", "numeric", "space", "printable")
ranges = {key: [] for key in KEYS}
active = {key: None for key in KEYS}
titlecase_map = []

def flush_key(key):
    item = active[key]
    if item is not None:
        ranges[key].append(item)
        active[key] = None

for code in range(0x110000):
    ch = chr(code)
    states = {}
    try:
        unicodedata.digit(ch)
        states["digit"] = True
    except ValueError:
        states["digit"] = False
    try:
        unicodedata.decimal(ch)
        states["decimal"] = True
    except ValueError:
        states["decimal"] = False
    try:
        unicodedata.numeric(ch)
        states["numeric"] = True
    except ValueError:
        states["numeric"] = False
    states["space"] = ch.isspace()
    states["printable"] = ch.isprintable()
    title = ch.title()
    upper = ch.upper()
    if title != upper:
        titlecase_cps = ",".join(str(ord(c)) for c in title)
        titlecase_map.append((code, titlecase_cps))

    for key in KEYS:
        is_member = states[key]
        item = active[key]
        if is_member:
            if item is None:
                active[key] = (code, code)
            else:
                lo, hi = item
                if code == hi + 1:
                    active[key] = (lo, code)
                else:
                    ranges[key].append(item)
                    active[key] = (code, code)
        elif item is not None:
            ranges[key].append(item)
            active[key] = None

for key in KEYS:
    flush_key(key)

print(unicodedata.unidata_version)
for key in KEYS:
    print(f"[{key}]")
    for lo, hi in ranges[key]:
        print(f"{lo},{hi}")
print("[titlecase]")
for code, cps in titlecase_map:
    print(f"{code};{cps}")
"#;
                stdin.write_all(script.as_bytes())?;
            }
            child.wait_with_output()
        });
    let output = match output {
        Ok(out) => out,
        Err(err) => {
            panic!("failed to run python3 to generate unicode digit ranges: {err}");
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("python3 unicode digit generation failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let version = lines.next().unwrap_or_default().trim().to_string();
    if version.is_empty() {
        panic!("python3 unicode category generation returned no version");
    }

    let mut digit_ranges = Vec::new();
    let mut decimal_ranges = Vec::new();
    let mut numeric_ranges = Vec::new();
    let mut space_ranges = Vec::new();
    let mut printable_ranges = Vec::new();
    let mut titlecase_entries: Vec<(u32, String)> = Vec::new();

    let mut current_section = "";
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() > 2 {
            current_section = &line[1..line.len() - 1];
            continue;
        }
        if current_section == "titlecase" {
            let Some((code, cps)) = line.split_once(';') else {
                continue;
            };
            let code: u32 = code.parse().expect("invalid unicode titlecase codepoint");
            let mut mapped = String::new();
            for cp in cps.split(',') {
                if cp.is_empty() {
                    continue;
                }
                let value: u32 = cp
                    .parse()
                    .expect("invalid unicode titlecase mapped codepoint");
                let Some(ch) = char::from_u32(value) else {
                    panic!("invalid unicode scalar in titlecase map: {value}");
                };
                mapped.push(ch);
            }
            if mapped.is_empty() {
                panic!("unicode titlecase mapping must not be empty for codepoint {code}");
            }
            titlecase_entries.push((code, mapped));
            continue;
        }
        let Some((lo, hi)) = line.split_once(',') else {
            continue;
        };
        let lo: u32 = lo.parse().expect("invalid unicode range start");
        let hi: u32 = hi.parse().expect("invalid unicode range end");
        match current_section {
            "digit" => digit_ranges.push((lo, hi)),
            "decimal" => decimal_ranges.push((lo, hi)),
            "numeric" => numeric_ranges.push((lo, hi)),
            "space" => space_ranges.push((lo, hi)),
            "printable" => printable_ranges.push((lo, hi)),
            _ => {}
        }
    }

    if digit_ranges.is_empty()
        || decimal_ranges.is_empty()
        || numeric_ranges.is_empty()
        || space_ranges.is_empty()
        || printable_ranges.is_empty()
        || titlecase_entries.is_empty()
    {
        panic!("python3 unicode category generation returned incomplete tables");
    }

    write_unicode_range_module(
        &out_dir,
        "unicode_digit_ranges.rs",
        "UNICODE_DIGIT_VERSION",
        "UNICODE_DIGIT_RANGES",
        &version,
        &digit_ranges,
    );
    write_unicode_range_module(
        &out_dir,
        "unicode_decimal_ranges.rs",
        "UNICODE_DECIMAL_VERSION",
        "UNICODE_DECIMAL_RANGES",
        &version,
        &decimal_ranges,
    );
    write_unicode_range_module(
        &out_dir,
        "unicode_numeric_ranges.rs",
        "UNICODE_NUMERIC_VERSION",
        "UNICODE_NUMERIC_RANGES",
        &version,
        &numeric_ranges,
    );
    write_unicode_range_module(
        &out_dir,
        "unicode_space_ranges.rs",
        "UNICODE_SPACE_VERSION",
        "UNICODE_SPACE_RANGES",
        &version,
        &space_ranges,
    );
    write_unicode_range_module(
        &out_dir,
        "unicode_printable_ranges.rs",
        "UNICODE_PRINTABLE_VERSION",
        "UNICODE_PRINTABLE_RANGES",
        &version,
        &printable_ranges,
    );
    write_unicode_titlecase_module(
        &out_dir,
        "unicode_titlecase_map.rs",
        "UNICODE_TITLECASE_VERSION",
        "UNICODE_TITLECASE_MAP",
        &version,
        &titlecase_entries,
    );
    println!("cargo:rerun-if-env-changed=PYTHONPATH");
    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    println!("cargo:rerun-if-changed=build.rs");
}

fn emit_native_cdylib_isolate_stubs(out_dir: &Path, target_arch: &str, target_env: &str) {
    if target_arch == "wasm32" {
        return;
    }

    let source = out_dir.join("molt_cdylib_isolate_stubs.c");
    // Mark the stubs as weak so they yield to strong definitions provided by
    // any downstream crate that links molt-runtime (e.g. molt-ffi's own
    // `runtime_linked` stubs, or a real isolate implementation in production
    // app code). Without weak linkage, building any cdylib that depends on
    // both molt-runtime and another crate that defines these symbols fails
    // with "duplicate symbol" at link time. `__attribute__((weak))` is
    // honored by clang on macOS, Linux/glibc/musl, and Windows MSVC; on
    // platforms where the compiler doesn't recognize it, the macro expands
    // to nothing and the stubs become regular strong symbols (matching the
    // pre-existing behavior).
    fs::write(
        &source,
        r#"#include <stdint.h>

#if defined(__GNUC__) || defined(__clang__)
#define MOLT_WEAK __attribute__((weak))
#else
#define MOLT_WEAK
#endif

MOLT_WEAK uint64_t molt_isolate_bootstrap(void) {
    return 0;
}

MOLT_WEAK uint64_t molt_isolate_import(uint64_t name_bits) {
    (void)name_bits;
    return 0;
}
"#,
    )
    .expect("failed to write native cdylib isolate stubs");

    let object_ext = if target_env == "msvc" { "obj" } else { "o" };
    let object = out_dir.join(format!("molt_cdylib_isolate_stubs.{object_ext}"));
    let compiler = Build::new().cargo_metadata(false).get_compiler();
    let mut cmd = compiler.to_command();
    if compiler.is_like_msvc() {
        cmd.arg("/nologo")
            .arg("/c")
            .arg(&source)
            .arg(format!("/Fo{}", object.display()));
    } else {
        cmd.arg("-c").arg(&source).arg("-o").arg(&object);
    }
    let status = cmd
        .status()
        .unwrap_or_else(|err| panic!("failed to compile native cdylib isolate stubs: {err}"));
    if !status.success() {
        panic!("compiling native cdylib isolate stubs failed: {status}");
    }
    println!("cargo:rustc-cdylib-link-arg={}", object.display());
}

fn write_unicode_range_module(
    out_dir: &Path,
    file_name: &str,
    version_symbol: &str,
    ranges_symbol: &str,
    version: &str,
    ranges: &[(u32, u32)],
) {
    let mut out = String::new();
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub(crate) const {version_symbol}: &str = \"{version}\";\n"
    ));
    out.push_str(&format!(
        "pub(crate) const {ranges_symbol}: &[(u32, u32)] = &[\n"
    ));
    for (lo, hi) in ranges {
        out.push_str(&format!("    ({lo}, {hi}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_dir.join(file_name), out).unwrap_or_else(|err| {
        panic!("failed to write {file_name}: {err}");
    });
}

fn write_unicode_titlecase_module(
    out_dir: &Path,
    file_name: &str,
    version_symbol: &str,
    map_symbol: &str,
    version: &str,
    entries: &[(u32, String)],
) {
    let mut out = String::new();
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub(crate) const {version_symbol}: &str = \"{version}\";\n"
    ));
    out.push_str(&format!(
        "pub(crate) const {map_symbol}: &[(u32, &str)] = &[\n"
    ));
    for (code, mapped) in entries {
        out.push_str(&format!("    ({code}, {mapped:?}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_dir.join(file_name), out).unwrap_or_else(|err| {
        panic!("failed to write {file_name}: {err}");
    });
}

fn build_libmpdec(
    manifest_dir: &Path,
    out_dir: &Path,
    target_env: &str,
    target_ptr_width: &str,
    target_arch: &str,
) -> bool {
    let repo_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("failed to locate repo root");
    let libmpdec_dir = repo_root.join("third_party/cpython/Modules/_decimal/libmpdec");
    let sources = [
        "basearith.c",
        "constants.c",
        "context.c",
        "convolute.c",
        "crt.c",
        "difradix2.c",
        "fnt.c",
        "fourstep.c",
        "io.c",
        "mpalloc.c",
        "mpdecimal.c",
        "numbertheory.c",
        "sixstep.c",
        "transpose.c",
    ];
    let headers = [
        "basearith.h",
        "bits.h",
        "constants.h",
        "convolute.h",
        "crt.h",
        "difradix2.h",
        "fnt.h",
        "fourstep.h",
        "io.h",
        "mpalloc.h",
        "mpdecimal.h",
        "numbertheory.h",
        "sixstep.h",
        "transpose.h",
        "typearith.h",
        "umodarith.h",
    ];

    for file in sources.iter().chain(headers.iter()) {
        println!(
            "cargo:rerun-if-changed={}",
            libmpdec_dir.join(file).display()
        );
    }

    let pyconfig = out_dir.join("pyconfig.h");
    if !pyconfig.exists() {
        fs::write(
            &pyconfig,
            "#ifndef Py_CONFIG_H\n#define Py_CONFIG_H\n#endif\n",
        )
        .expect("failed to write stub pyconfig.h");
    }
    println!("cargo:rerun-if-changed={}", pyconfig.display());

    let missing: Vec<String> = sources
        .iter()
        .chain(headers.iter())
        .map(|name| libmpdec_dir.join(name))
        .filter(|path| !path.exists())
        .map(|path| path.display().to_string())
        .collect();
    if !missing.is_empty() {
        return false;
    }

    let mut build = Build::new();
    build.include(&libmpdec_dir);
    build.include(out_dir);
    for src in sources {
        build.file(libmpdec_dir.join(src));
    }
    build.flag_if_supported("-std=c99");
    build.define("ANSI", "1");
    if target_ptr_width == "64" {
        build.define("CONFIG_64", "1");
        if target_env != "msvc" {
            build.define("HAVE_UINT128_T", "1");
        }
    } else {
        build.define("CONFIG_32", "1");
    }
    if target_arch == "wasm32" {
        build.define("_WASI_EMULATED_SIGNAL", "1");
        let mut wasi_sysroot = env::var("WASI_SYSROOT")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                env::var("WASI_SDK_PATH")
                    .ok()
                    .map(|sdk_root| PathBuf::from(sdk_root).join("share").join("wasi-sysroot"))
            });
        if wasi_sysroot.is_none() {
            let candidates = [
                "/opt/homebrew/opt/wasi-libc/share/wasi-sysroot",
                "/usr/local/opt/wasi-libc/share/wasi-sysroot",
            ];
            for candidate in candidates {
                let path = PathBuf::from(candidate);
                if path.exists() {
                    wasi_sysroot = Some(path);
                    break;
                }
            }
        }
        let Some(sysroot) = wasi_sysroot else {
            panic!(
                "WASI sysroot not found: set WASI_SYSROOT or WASI_SDK_PATH, \
                 or install wasi-libc (Homebrew) so wasm32-wasip1 builds can compile."
            );
        };
        build.flag(format!("--sysroot={}", sysroot.display()));
        let lib_path = sysroot.join("lib").join("wasm32-wasip1");
        println!("cargo:rustc-link-search=native={}", lib_path.display());
        println!("cargo:rustc-link-lib=wasi-emulated-signal");
    }
    build.compile("molt_mpdec");
    true
}

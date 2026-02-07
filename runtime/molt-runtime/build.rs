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

    if target_os == "macos" {
        println!("cargo:rustc-link-arg-cdylib=-Wl,-undefined,dynamic_lookup");
    }

    build_libmpdec(
        &manifest_dir,
        &out_dir,
        &target_env,
        &target_ptr_width,
        &target_arch,
    );

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
ranges = []
start = None
end = None
for code in range(0x110000):
    ch = chr(code)
    try:
        unicodedata.digit(ch)
        is_digit = True
    except ValueError:
        is_digit = False
    if is_digit:
        if start is None:
            start = code
            end = code
        elif code == end + 1:
            end = code
        else:
            ranges.append((start, end))
            start = code
            end = code
    elif start is not None:
        ranges.append((start, end))
        start = None
        end = None
if start is not None:
    ranges.append((start, end))
print(unicodedata.unidata_version)
for lo, hi in ranges:
    print(f"{lo},{hi}")
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
        panic!("python3 unicode digit generation returned no version");
    }
    let mut ranges = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((lo, hi)) = line.split_once(',') else {
            continue;
        };
        let lo: u32 = lo.parse().expect("invalid unicode digit range start");
        let hi: u32 = hi.parse().expect("invalid unicode digit range end");
        ranges.push((lo, hi));
    }
    if ranges.is_empty() {
        panic!("python3 unicode digit generation returned no ranges");
    }
    let mut out = String::new();
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub(crate) const UNICODE_DIGIT_VERSION: &str = \"{version}\";\n"
    ));
    out.push_str("pub(crate) const UNICODE_DIGIT_RANGES: &[(u32, u32)] = &[\n");
    for (lo, hi) in ranges {
        out.push_str(&format!("    ({lo}, {hi}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_dir.join("unicode_digit_ranges.rs"), out)
        .expect("failed to write unicode_digit_ranges.rs");
    println!("cargo:rerun-if-env-changed=PYTHONPATH");
    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    println!("cargo:rerun-if-changed=build.rs");
}

fn build_libmpdec(
    manifest_dir: &Path,
    out_dir: &Path,
    target_env: &str,
    target_ptr_width: &str,
    target_arch: &str,
) {
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
}

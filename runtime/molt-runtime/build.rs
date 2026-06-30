use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cc::Build;

#[path = "../build_support/unicode_tables.rs"]
mod unicode_tables;
#[path = "../build_support/wasi_sysroot.rs"]
mod wasi_sysroot;

fn resolve_build_python() -> String {
    println!("cargo:rerun-if-env-changed=MOLT_BUILD_PYTHON");
    println!("cargo:rerun-if-env-changed=PYTHON");
    for key in ["MOLT_BUILD_PYTHON", "PYTHON"] {
        if let Ok(value) = env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let target_ptr_width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let build_python = resolve_build_python();

    // Keep cdylib in the crate types so plain `cargo build -p molt-runtime`
    // still emits a stable `molt_runtime.wasm` artifact for wasm lanes that
    // consume the runtime directly.
    let _ = &target_os;
    println!("cargo:rustc-check-cfg=cfg(molt_has_mpdec)");

    // Emit `molt_has_net_io` only when the target has Molt's native socket ABI
    // implementation, not merely because the stdlib_net Cargo feature was
    // requested. WASM uses the host-call socket ABI under target_arch = "wasm32";
    // Windows stays on the explicit no-net intrinsic surface until the WinSock
    // constants, sockaddr storage, resolver, SSL fd ownership, and poller
    // contracts land as one coherent target implementation.
    println!("cargo:rustc-check-cfg=cfg(molt_has_net_io)");
    let native_net_target_supported =
        target_arch != "wasm32" && target_family.split(',').any(|family| family == "unix");
    if native_net_target_supported {
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
        let output = Command::new(&build_python)
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
                panic!(
                    "failed to run build Python `{build_python}` to generate errno constants: {err}"
                );
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("build Python `{build_python}` errno generation failed: {stderr}");
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
            panic!("build Python `{build_python}` errno generation returned no entries");
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

    unicode_tables::emit_runtime_unicode_tables(&out_dir, &build_python);
    println!("cargo:rerun-if-env-changed=PYTHONPATH");
    println!("cargo:rerun-if-changed=../build_support/unicode_tables.rs");
    println!("cargo:rerun-if-changed=../build_support/wasi_sysroot.rs");
    println!("cargo:rerun-if-changed=src/object/ops.rs");
    println!("cargo:rerun-if-changed=build.rs");
}

fn emit_native_cdylib_isolate_stubs(out_dir: &Path, target_arch: &str, target_env: &str) {
    if target_arch == "wasm32" {
        return;
    }

    let source = out_dir.join("molt_cdylib_isolate_stubs.c");
    // Provide unresolved-symbol fallbacks that yield to strong definitions from
    // downstream crates, integration tests, or production app code. GNU/Clang
    // targets can use weak definitions directly. MSVC needs `/alternatename`
    // aliases so linking the fallback object into every test binary does not
    // collide with tests that provide their own isolate symbols.
    fs::write(
        &source,
        r#"#include <stdint.h>

#if defined(_MSC_VER)
uint64_t molt_isolate_bootstrap_stub(void) {
    return 0;
}

uint64_t molt_isolate_import_stub(uint64_t name_bits) {
    (void)name_bits;
    return 0;
}

#pragma comment(linker, "/alternatename:molt_isolate_bootstrap=molt_isolate_bootstrap_stub")
#pragma comment(linker, "/alternatename:molt_isolate_import=molt_isolate_import_stub")
#elif defined(__GNUC__) || defined(__clang__)
#define MOLT_WEAK __attribute__((weak))

MOLT_WEAK uint64_t molt_isolate_bootstrap(void) {
    return 0;
}

MOLT_WEAK uint64_t molt_isolate_import(uint64_t name_bits) {
    (void)name_bits;
    return 0;
}
#else
uint64_t molt_isolate_bootstrap(void) {
    return 0;
}

uint64_t molt_isolate_import(uint64_t name_bits) {
    (void)name_bits;
    return 0;
}
#endif
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
    println!("cargo:rustc-link-arg-tests={}", object.display());
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
        let Some(sysroot) = wasi_sysroot::resolve_wasi_sysroot() else {
            panic!(
                "WASI sysroot not found: set MOLT_WASI_SYSROOT, WASI_SYSROOT, \
                 WASI_SDK_PATH, WASI_SDK_PREFIX, or MOLT_TARGET_ROOT so \
                 wasm32-wasip1 runtime C shims can compile."
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

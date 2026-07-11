use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use cc::Build;

#[path = "../build_support/unicode_tables.rs"]
mod unicode_tables;
#[path = "../build_support/variadic_exports.rs"]
mod variadic_exports;
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
    emit_cpython_abi_variadic_export_anchors(&manifest_dir, &out_dir);

    // Keep cdylib in the crate types so plain `cargo build -p molt-runtime`
    // still emits a stable `molt_runtime.wasm` artifact for wasm lanes that
    // consume the runtime directly.
    let _ = &target_os;
    println!("cargo:rustc-check-cfg=cfg(molt_has_mpdec)");
    emit_cpython_abi_requested_export_anchors(&out_dir, &target_arch);

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

    emit_wasm_long_double_link_policy(&out_dir, &target_arch);

    unicode_tables::emit_runtime_unicode_tables(&out_dir, &build_python);
    println!("cargo:rerun-if-env-changed=PYTHONPATH");
    println!("cargo:rerun-if-changed=../build_support/unicode_tables.rs");
    println!("cargo:rerun-if-changed=../build_support/wasi_sysroot.rs");
    println!("cargo:rerun-if-changed=../build_support/variadic_exports.rs");
    println!("cargo:rerun-if-changed=src/object/ops.rs");
    println!("cargo:rerun-if-changed=build.rs");
}

fn emit_cpython_abi_variadic_export_anchors(manifest_dir: &Path, out_dir: &Path) {
    let manifest = manifest_dir.join("../molt-cpython-abi/shims/pyarg_variadic.exports");
    let symbols = variadic_exports::load_variadic_exports(&manifest);
    let output = out_dir.join("molt_cpython_abi_variadic_exports.rs");
    fs::write(&output, variadic_exports::render_rust_anchors(&symbols))
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", output.display()));
    println!("cargo:rerun-if-changed={}", manifest.display());
}

fn emit_cpython_abi_requested_export_anchors(out_dir: &Path, target_arch: &str) {
    println!("cargo:rerun-if-env-changed=MOLT_WASM_CPYTHON_ABI_EXPORTS");
    println!("cargo:rerun-if-env-changed=MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS");
    let output = out_dir.join("molt_cpython_abi_requested_exports.rs");
    let raw = env::var("MOLT_WASM_CPYTHON_ABI_EXPORTS").unwrap_or_default();
    let raw_data = env::var("MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS").unwrap_or_default();
    let mut function_symbols = Vec::new();
    let mut data_symbols = Vec::new();
    if target_arch == "wasm32" {
        let requested_symbols = parse_cpython_abi_export_symbol_list(&raw);
        let requested_data_symbols = parse_cpython_abi_export_symbol_list(&raw_data);
        for symbol in &requested_data_symbols {
            if !requested_symbols.contains(symbol) {
                panic!(
                    "CPython ABI data export symbol {symbol} was not requested for WASM runtime"
                );
            }
        }
        for symbol in requested_symbols {
            if requested_data_symbols.contains(&symbol) {
                data_symbols.push(symbol.to_string());
            } else {
                function_symbols.push(symbol.to_string());
            }
        }
        function_symbols.sort();
        function_symbols.dedup();
        data_symbols.sort();
        data_symbols.dedup();
    }

    let mut source = String::new();
    source.push_str("// @generated by runtime/molt-runtime/build.rs\n");
    if !function_symbols.is_empty() || !data_symbols.is_empty() {
        source.push_str("unsafe extern \"C\" {\n");
        for symbol in &function_symbols {
            source.push_str(&format!("    fn {symbol}();\n"));
        }
        for symbol in &data_symbols {
            source.push_str(&format!("    static mut {symbol}: u8;\n"));
        }
        source.push_str("}\n\n");
    }
    source.push_str(&format!(
        "#[used]\nstatic MOLT_CPYTHON_ABI_REQUESTED_FUNCTION_EXPORT_ANCHORS: [unsafe extern \"C\" fn(); {}] = [\n",
        function_symbols.len()
    ));
    for symbol in &function_symbols {
        source.push_str(&format!("    {symbol},\n"));
    }
    source.push_str("];\n\n");
    source.push_str("pub(super) fn requested_export_anchor_count() -> usize {\n");
    source.push_str(
        "    core::hint::black_box(MOLT_CPYTHON_ABI_REQUESTED_FUNCTION_EXPORT_ANCHORS.as_ptr());\n",
    );
    source.push_str(
        "    let mut count = MOLT_CPYTHON_ABI_REQUESTED_FUNCTION_EXPORT_ANCHORS.len();\n",
    );
    if !data_symbols.is_empty() {
        source.push_str("    unsafe {\n");
        for symbol in &data_symbols {
            source.push_str(&format!(
                "        core::hint::black_box(&raw mut {symbol});\n        count += 1;\n"
            ));
        }
        source.push_str("    }\n");
    }
    source.push_str("    count\n");
    source.push_str("}\n");
    fs::write(output, source).expect("failed to write CPython ABI WASM export anchors");
}

fn parse_cpython_abi_export_symbol_list(raw: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for symbol in raw.split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace()) {
        let symbol = symbol.trim();
        if symbol.is_empty() {
            continue;
        }
        if !is_c_identifier(symbol) {
            panic!("invalid CPython ABI export symbol requested for WASM runtime: {symbol}");
        }
        symbols.push(symbol.to_string());
    }
    symbols.sort();
    symbols.dedup();
    symbols
}

fn is_c_identifier(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Single authority (deploy-cdylib arm) for wasi-libc's `long double` (`%L`)
/// printf/scanf link policy — the SAME policy the molt-driven `wasm-ld` links
/// (reloc runtime, split `app.wasm`) apply via `molt.cli` Python helpers.
///
/// The default wasi-libc `libc.a` stubs the `%L` float conversions with a
/// `long_double_not_supported()` that `abort()`s -> raw `unreachable` trap
/// (numpy `_multiarray_umath` import hits it). wasi-libc ships the real
/// formatters in a companion archive `libc-printscan-long-double.a` whose real
/// `vfprintf`/`__floatscan`/`strtold` override the stub *when linked ahead of
/// `libc.a`*. The reloc/app hand-links whole-archive it before `libc.a`; the
/// deploy `cdylib` is linked by rustc, which places the self-contained `-lc`
/// AFTER any `-C link-arg`, so a trailing `-lc-printscan-long-double` is too
/// late (the stub is already pulled). A build-script `cargo:rustc-link-lib`,
/// however, is emitted in rustc's *local native libraries* group, which
/// precedes the self-contained sysroot `-lc`: linking the real formatters as a
/// normal (lazy, un-bundled) static lib there pulls `printscan`'s `vfprintf.o`
/// to satisfy molt's own `PyOS_snprintf`/`vfprintf` reference FIRST, so
/// `libc.a`'s stub object stays lazy and is never linked. Normal (not
/// `--whole-archive`) can never duplicate-symbol; `-bundle` keeps the archive
/// out of the sibling `staticlib`/`rlib` crate-types (the reloc runtime whole-
/// archives its own copy, so bundling would double-define). The
/// `artifact_poison_gate` attests the effect (stub string ABSENT) uniformly
/// across all three built artifacts.
///
/// Archive identities come from the molt Python resolver via env
/// (`MOLT_WASM_LONGDOUBLE_ARCHIVE` / `MOLT_WASM_BUILTINS_ARCHIVE` — the single
/// source of truth, incl. the vendored fallback), with a wasi-sysroot lookup
/// fallback for a plain `cargo build -p molt-runtime`.
fn emit_wasm_long_double_link_policy(out_dir: &Path, target_arch: &str) {
    println!("cargo:rerun-if-env-changed=MOLT_WASM_LONGDOUBLE_ARCHIVE");
    println!("cargo:rerun-if-env-changed=MOLT_WASM_BUILTINS_ARCHIVE");
    if target_arch != "wasm32" {
        return;
    }
    let printscan = resolve_wasm_link_archive(
        "MOLT_WASM_LONGDOUBLE_ARCHIVE",
        "libc-printscan-long-double.a",
    );
    let Some(printscan) = printscan else {
        // No archive resolvable: emit nothing. numpy/scipy-tier builds fail loud
        // upstream in molt.cli (`_resolve_reloc_long_double_archives`); a micro
        // build never hits `%L`, so leaving the (unreachable) stub is benign.
        return;
    };
    let printscan_dst = out_dir.join("libc-printscan-long-double.a");
    if let Err(err) = fs::copy(&printscan, &printscan_dst) {
        panic!(
            "failed to stage wasi-libc long-double printf/scanf archive {} -> {}: {err}",
            printscan.display(),
            printscan_dst.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // Normal (lazy) + un-bundled: overrides the stub without dup, and stays out
    // of the staticlib/rlib the reloc runtime consumes.
    println!("cargo:rustc-link-lib=static:-bundle=c-printscan-long-double");
    // binary128 soft-float (__addtf3/__multf3/…) the real long-double path calls.
    if let Some(builtins) = resolve_wasm_link_archive(
        "MOLT_WASM_BUILTINS_ARCHIVE",
        "libclang_rt.builtins-wasm32.a",
    ) {
        let builtins_dst = out_dir.join("libclang_rt.builtins-wasm32.a");
        if let Err(err) = fs::copy(&builtins, &builtins_dst) {
            panic!(
                "failed to stage compiler-rt builtins archive {} -> {}: {err}",
                builtins.display(),
                builtins_dst.display()
            );
        }
        println!("cargo:rustc-link-lib=static:-bundle=clang_rt.builtins-wasm32");
    }
}

/// Resolve a wasm link archive: molt-provided env path first (the Python
/// resolver, incl. vendored fallback), then the active wasi-sysroot lib dir.
fn resolve_wasm_link_archive(env_key: &str, file_name: &str) -> Option<PathBuf> {
    if let Ok(value) = env::var(env_key) {
        let value = value.trim();
        if !value.is_empty() {
            let path = PathBuf::from(value);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let sysroot = wasi_sysroot::resolve_wasi_sysroot()?;
    let candidate = sysroot.lib_dir("wasm32-wasip1").join(file_name);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
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
        build.flag(sysroot.sysroot_flag());
        if let Some(include_dir) = sysroot.include_dir() {
            build.include(include_dir);
        }
        let lib_path = sysroot.lib_dir("wasm32-wasip1");
        println!("cargo:rustc-link-search=native={}", lib_path.display());
        println!("cargo:rustc-link-lib=wasi-emulated-signal");
    }
    build.compile("molt_mpdec");
    true
}

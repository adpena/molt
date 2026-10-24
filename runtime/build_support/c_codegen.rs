//! C codegen policy shared by every Molt build script that compiles C.
//!
//! One home for the flags that depend on the target's object format, so no
//! build script carries its own copy of the rule or warns on a host where
//! the flag has no meaning.

use std::path::Path;

/// Targets whose shared objects use ELF symbol interposition.
///
/// `-fno-semantic-interposition` only has meaning there. Apple clang (Mach-O)
/// and the wasm toolchains accept the flag but warn that it is unused, and
/// MSVC (PE/COFF) has no such concept, so the flag is applied by object
/// format rather than by "not macOS".
pub fn target_uses_elf(target_os: &str) -> bool {
    matches!(
        target_os,
        "linux"
            | "android"
            | "freebsd"
            | "netbsd"
            | "openbsd"
            | "dragonfly"
            | "solaris"
            | "illumos"
            | "haiku"
            | "fuchsia"
    )
}

/// Apply the shared optimisation policy to one C translation-unit build.
pub fn apply_codegen_policy(
    build: &mut cc::Build,
    target_os: &str,
    manifest_dir: &Path,
    out_dir: &Path,
) {
    build.opt_level(3);
    if target_uses_elf(target_os) {
        build.flag_if_supported("-fno-semantic-interposition");
    }
    apply_source_path_policy(build, manifest_dir, out_dir);
}

/// Remap compiler-owned source metadata, without rewriting ambient CFLAGS.
///
/// Cargo owns both arguments; the source closure owns this implementation.
/// Call after selecting the final target compiler, including the WASI driver.
/// These are direct compiler argv, so unlike cmake-rs's joined CFLAGS they
/// preserve paths with spaces without process-global CL environment changes.
pub fn apply_source_path_policy(build: &mut cc::Build, manifest_dir: &Path, out_dir: &Path) {
    let source_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("Molt C package must be under runtime/<package>");
    let compiler = build.get_compiler();
    let mappings = [(source_root, "/molt/source"), (out_dir, "/molt/generated")];
    if compiler.is_like_msvc() && !compiler.is_like_clang_cl() {
        // MSVC ignores /pathmap without its deterministic mode. Retain debug
        // information; neither /DEBUG:NONE nor stripping is a substitute.
        build.flag("/experimental:deterministic");
        for (from, to) in mappings {
            build.flag(format!("/pathmap:{}={to}", from.display()));
        }
    } else {
        for (from, to) in mappings {
            for option in ["-ffile-prefix-map", "-fdebug-prefix-map"] {
                let argument = format!("{option}={}={to}", from.display());
                build.flag(if compiler.is_like_clang_cl() {
                    format!("/clang:{argument}")
                } else {
                    argument
                });
            }
        }
    }
}

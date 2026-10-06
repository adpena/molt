//! C codegen policy shared by every Molt build script that compiles C.
//!
//! One home for the flags that depend on the target's object format, so no
//! build script carries its own copy of the rule or warns on a host where
//! the flag has no meaning.

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
pub fn apply_codegen_policy(build: &mut cc::Build, target_os: &str) {
    build.opt_level(3);
    if target_uses_elf(target_os) {
        build.flag_if_supported("-fno-semantic-interposition");
    }
}

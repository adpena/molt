from __future__ import annotations

from pathlib import Path
import re
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def _load_workspace_manifest() -> dict[str, object]:
    with (ROOT / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def _load_backend_manifest() -> dict[str, object]:
    with (ROOT / "runtime" / "molt-backend" / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def test_backend_manifest_does_not_depend_on_obj_model() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]
    assert "molt-obj-model" not in dependencies


def test_backend_manifest_does_not_redeclare_wasmparser_in_dev_dependencies() -> None:
    manifest = _load_backend_manifest()
    dev_dependencies = manifest.get("dev-dependencies", {})
    assert "wasmparser" not in dev_dependencies


def test_backend_manifest_uses_serde_with_derive_feature() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]

    # serde is required for JSON boundary, IR serialization, and TIR
    assert "serde" in dependencies
    serde_dep = dependencies["serde"]
    assert "derive" in serde_dep.get("features", [])


def test_backend_manifest_uses_minimal_cranelift_codegen_features() -> None:
    manifest = _load_backend_manifest()
    codegen_dependency = manifest["dependencies"]["cranelift-codegen"]

    assert codegen_dependency["default-features"] is False
    assert set(codegen_dependency["features"]) == {"host-arch", "std", "unwind"}


def test_backend_manifest_target_overlays_only_add_cross_isa_support() -> None:
    manifest = _load_backend_manifest()
    target_tables = manifest["target"]
    aarch64_dependency = target_tables['cfg(target_arch = "aarch64")']["dependencies"][
        "cranelift-codegen"
    ]
    x86_64_dependency = target_tables['cfg(target_arch = "x86_64")']["dependencies"][
        "cranelift-codegen"
    ]

    assert aarch64_dependency["default-features"] is False
    assert aarch64_dependency["features"] == ["x86"]
    assert x86_64_dependency["default-features"] is False
    assert x86_64_dependency["features"] == ["arm64"]


def test_workspace_dev_profile_trims_backend_debug_info() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    dev_packages = profiles["dev"]["package"]
    dev_fast_packages = profiles["dev-fast"]["package"]
    expected_packages = {
        "molt-backend",
        "cranelift-codegen",
        "cranelift-frontend",
        "gimli",
        "cranelift-module",
        "cranelift-native",
        "cranelift-object",
        "object",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0


def test_workspace_dev_profile_trims_runtime_debug_info() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    dev_packages = profiles["dev"]["package"]
    dev_fast_packages = profiles["dev-fast"]["package"]
    expected_packages = {
        "molt-runtime",
        "aws-lc-rs",
        "aws-lc-sys",
        "httparse",
        "libbz2-rs-sys",
        "proc-macro2",
        "quote",
        "rustpython-parser",
        "rustpython-ast",
        "rustpython-parser-core",
        "rustls",
        "rustls-pemfile",
        "rustls-webpki",
        "serde",
        "serde_core",
        "serde_json",
        "simdutf",
        "syn",
        "thiserror",
        "tungstenite",
        "unicode_names2",
        "url",
        "webpki-roots",
        "xz2",
        "zlib-rs",
        "lzma-sys",
        "zip",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0


def test_workspace_dev_fast_does_not_force_opt_level() -> None:
    manifest = _load_workspace_manifest()
    dev_fast_profile = manifest["profile"]["dev-fast"]

    assert "opt-level" not in dev_fast_profile


def test_runtime_manifest_uses_flate2_zip_deflate_only() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    zip_dependency = runtime_manifest["dependencies"]["zip"]

    assert zip_dependency["default-features"] is False
    assert zip_dependency["features"] == ["deflate"]


def test_runtime_net_io_cfg_requires_supported_native_socket_abi() -> None:
    build_rs = (ROOT / "runtime" / "molt-runtime" / "build.rs").read_text()
    net_stubs = (
        ROOT / "runtime" / "molt-runtime" / "src" / "async_rt" / "net_stubs.rs"
    ).read_text()

    assert 'env::var("CARGO_CFG_TARGET_FAMILY")' in build_rs
    assert 'let native_net_target_supported = target_arch != "wasm32"' in build_rs
    assert 'family == "unix"' in build_rs
    assert "if native_net_target_supported" in build_rs
    assert 'println!("cargo:rustc-cfg=molt_has_net_io")' in build_rs

    assert '#[cfg(feature = "stdlib_net")]' in net_stubs
    assert "networking not available for this runtime target" in net_stubs
    assert "networking not available (compile with stdlib_net)" in net_stubs


def test_runtime_manifest_uses_minimal_rustpython_parser_features() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    parser_dependency = runtime_manifest["dependencies"]["rustpython-parser"]

    assert parser_dependency["default-features"] is False
    assert set(parser_dependency["features"]) == {"location", "num-bigint"}


def test_runtime_manifest_dedupes_unicode_names2_version() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    text_manifest_path = ROOT / "runtime" / "molt-runtime-text" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    with text_manifest_path.open("rb") as handle:
        text_manifest = tomllib.load(handle)

    runtime_dep = runtime_manifest["dependencies"]["unicode_names2"]
    text_dep = text_manifest["dependencies"]["unicode_names2"]
    runtime_version = (
        runtime_dep["version"] if isinstance(runtime_dep, dict) else runtime_dep
    )
    text_version = text_dep["version"] if isinstance(text_dep, dict) else text_dep

    assert runtime_version == text_version == "2.0"


def test_runtime_manifest_declares_vfs_bundle_tar_feature() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    assert "vfs_bundle_tar" in runtime_manifest["features"]


def test_runtime_tk_native_feature_is_owned_by_leaf_crate() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    tk_manifest_path = ROOT / "runtime" / "molt-runtime-tk" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    with tk_manifest_path.open("rb") as handle:
        tk_manifest = tomllib.load(handle)

    runtime_features = runtime_manifest["features"]
    tk_dependency = runtime_manifest["dependencies"]["molt-runtime-tk"]
    native_target_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))'][
        "dependencies"
    ]

    assert tk_dependency["default-features"] is False
    assert "libloading" not in runtime_manifest["dependencies"]
    assert "libloading" not in native_target_deps
    assert runtime_features["stdlib_tk"] == [
        "dep:molt-runtime-tk",
        "molt-runtime-tk/tk",
    ]
    assert runtime_features["molt_tk_native"] == [
        "stdlib_tk",
        "molt-runtime-tk/native-tcl",
    ]
    assert tk_manifest["features"]["native-tcl"] == ["tk", "dep:libloading"]


def test_runtime_micro_profile_includes_core_non_network_intrinsics() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    micro_features = runtime_manifest["features"]["stdlib_micro"]

    assert micro_features == [
        "stdlib_asyncio",
        "stdlib_collections",
        "stdlib_fs_extra",
        "stdlib_logging",
        "stdlib_logging_ext",
    ]
    assert "stdlib_net" not in micro_features


def test_cli_micro_base_mirror_does_not_drift_from_cargo_stdlib_micro() -> None:
    """The CLI profile-availability mirror must equal Cargo.toml ``stdlib_micro``.

    ``_MICRO_BASE_RUNTIME_FEATURES`` is a hand-maintained Python mirror of the
    Cargo ``stdlib_micro`` feature list, which is the always-linked base of every
    profile (strict superset chain micro→edge→standard→server→full).  When the
    mirror omits a feature ``stdlib_micro`` pulls in, the compile-time
    profile-availability gate falsely refuses any import graph that statically
    reaches that feature's intrinsics.  That exact drift (the mirror omitted
    ``stdlib_collections``) silently broke ``import pprint`` / ``import asyncio``
    the moment P0 #50 made class-body control flow execute.  This guard turns
    the drift into a CI failure instead of a latent silent refusal (task #85).
    """
    from molt.cli.runtime_features import _MICRO_BASE_RUNTIME_FEATURES

    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    micro_features = runtime_manifest["features"]["stdlib_micro"]

    assert set(_MICRO_BASE_RUNTIME_FEATURES) == set(micro_features), (
        "CLI _MICRO_BASE_RUNTIME_FEATURES drifted from Cargo.toml stdlib_micro: "
        f"cli={sorted(_MICRO_BASE_RUNTIME_FEATURES)} cargo={sorted(micro_features)}"
    )


def test_cli_profile_availability_covers_every_always_linked_micro_feature() -> None:
    """Every profile/target enabled set must cover the always-linked micro base.

    ``stdlib_micro`` is linked into EVERY profile archive, so the
    profile-availability gate must never exclude a micro-base feature for any
    profile or target — doing so falsely refuses builds whose import graph
    reaches those intrinsics.  This pins the invariant across the three branches
    of ``_runtime_builtin_features_for_profile`` (non-micro / micro-wasm /
    micro-native).
    """
    from molt.cli.runtime_features import (
        _MICRO_BASE_RUNTIME_FEATURES,
        _runtime_builtin_features_for_profile,
    )

    micro_base = set(_MICRO_BASE_RUNTIME_FEATURES)
    for profile in (None, "full", "server", "micro"):
        for target in (None, "aarch64-apple-darwin", "wasm32-unknown-unknown"):
            enabled = set(
                _runtime_builtin_features_for_profile(profile, target_triple=target)
            )
            assert micro_base <= enabled, (
                f"profile={profile!r} target={target!r} omits always-linked "
                f"micro-base feature(s) {sorted(micro_base - enabled)}"
            )


def test_runtime_micro_tls_from_fd_stub_matches_intrinsic_arity() -> None:
    manifest_source = (
        ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "manifest.pyi"
    ).read_text()
    generated_source = (
        ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "generated.rs"
    ).read_text()
    stub_source = (
        ROOT / "runtime" / "molt-runtime" / "src" / "async_rt" / "net_stubs.rs"
    ).read_text()

    assert (
        "def molt_asyncio_tls_client_from_fd_new(\n"
        "    fd: int, server_hostname: str | None = ...\n"
        ") -> Any: ..."
    ) in manifest_source
    assert (
        'name: "molt_asyncio_tls_client_from_fd_new",\n'
        '        symbol: "molt_asyncio_tls_client_from_fd_new",\n'
        "        arity: 2,"
    ) in generated_source
    stub_signature = re.search(
        r"fn molt_asyncio_tls_client_from_fd_new\(([^)]*)\) -> u64",
        stub_source,
        re.MULTILINE,
    )
    assert stub_signature is not None
    assert stub_signature.group(1).count(": u64") == 2


def test_runtime_manifest_crate_types_include_all_link_targets() -> None:
    """Validate the runtime ships staticlib + rlib + cdylib.

    cdylib is required so ``cargo build -p molt-runtime --target wasm32-…``
    emits a stable ``.wasm`` artifact consumed by the WASM split-runtime lane.
    See ``build.rs`` for the authoritative rationale.
    """
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    crate_types = runtime_manifest["lib"]["crate-type"]

    assert "staticlib" in crate_types
    assert "rlib" in crate_types
    assert "cdylib" in crate_types


def test_backend_manifest_gates_loop_continue_to_native_backend() -> None:
    manifest = _load_backend_manifest()
    tests = manifest.get("test", [])
    loop_continue = next(test for test in tests if test["name"] == "loop_continue")
    assert loop_continue["required-features"] == ["native-backend"]


def test_runtime_manifest_avoids_url_compile_graph_for_websocket_client() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    native_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))'][
        "dependencies"
    ]

    assert "url" not in native_deps
    assert "url" not in native_deps["tungstenite"]["features"]


def test_backend_ir_model_and_passes_are_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()
    ir_rs = ROOT / "runtime" / "molt-backend" / "src" / "ir.rs"
    passes_rs = ROOT / "runtime" / "molt-backend" / "src" / "passes.rs"

    assert ir_rs.exists()
    assert passes_rs.exists()
    assert "pub struct SimpleIR" not in lib_rs
    assert "pub fn validate_simple_ir" not in lib_rs


def test_backend_native_trampoline_identity_is_split_out_of_lib_rs() -> None:
    native_backend_mod_path = (
        ROOT / "runtime" / "molt-backend" / "src" / "native_backend" / "mod.rs"
    )
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()

    assert native_backend_mod_path.exists()
    # The god-file split extracted trampoline identity OUT of the lib.rs facade
    # and into the native_backend module (struct TrampolineKey lives in mod.rs,
    # not a standalone trampolines.rs).  Assert the real current home so the
    # guard pins the actual structure rather than a renamed-away filename.
    assert "struct TrampolineKey" not in lib_rs
    assert "struct TrampolineKey" in native_backend_mod_path.read_text()


def test_backend_native_compile_func_is_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()
    function_compiler_rs = (
        ROOT
        / "runtime"
        / "molt-backend"
        / "src"
        / "native_backend"
        / "function_compiler.rs"
    )

    assert function_compiler_rs.exists()
    assert "fn compile_func(" not in lib_rs


def test_native_backend_codegen_failures_are_fail_closed() -> None:
    native_sources = [
        ROOT / "runtime" / "molt-backend" / "src" / "lib.rs",
        ROOT
        / "runtime"
        / "molt-backend"
        / "src"
        / "native_backend"
        / "function_compiler.rs",
    ]
    combined = "\n".join(path.read_text() for path in native_sources)

    assert "catch_unwind" not in combined
    assert "emit_trap_stub" not in combined
    assert "trap_stub_names" not in combined
    assert "emitting trap stub" not in combined
    assert "will retry at opt_level=none" not in combined

import os

import pytest
from pathlib import Path
import sys

from molt import rust_toolchain

from molt.rust_toolchain import (
    RustToolSearch,
    cargo_config_arguments,
    cargo_configuration_paths,
    relative_rustc_tool_paths,
    rustc_host,
    rustc_printed_sysroot,
)
from tests.process_guard_common import install_module_view


@pytest.mark.parametrize("cargo_home", [None, "", "relative-cargo", "~/literal-cargo"])
def test_cargo_configuration_discovery_has_one_home_and_shadowing_authority(
    tmp_path, cargo_home
):
    root = tmp_path / "project"
    profile = tmp_path / "profile"
    env = {"USERPROFILE" if os.name == "nt" else "HOME": str(profile)}
    home = profile / ".cargo" if not cargo_home else root / cargo_home
    if cargo_home is not None:
        env["CARGO_HOME"] = cargo_home
    home.mkdir(parents=True)
    home_config = home / "config.toml"
    home_config.write_text("[env]\n", encoding="utf-8")
    local = root / ".cargo"
    local.mkdir(parents=True)
    chosen = local / "config"
    chosen.write_text("[build]\n", encoding="utf-8")
    (local / "config.toml").write_text("[build]\n", encoding="utf-8")
    paths = cargo_configuration_paths(root, env)
    assert home_config.resolve() in paths
    assert chosen.resolve() in paths
    assert paths.index(home_config.resolve()) < paths.index(chosen.resolve())
    assert (local / "config.toml").resolve() not in paths


@pytest.mark.parametrize(
    "host,suffix",
    [
        ("x86_64-pc-windows-msvc", ".exe"),
        ("x86_64-unknown-linux-gnu", ""),
        ("aarch64-apple-darwin", ""),
    ],
)
def test_rust_bundled_linker_uses_compiler_host_not_wasm_target(tmp_path, host, suffix):
    selected, compiler = tmp_path / "selected", tmp_path / "compiler"
    for root in (selected, compiler):
        tool = root / "lib" / "rustlib" / host / "bin" / ("rust-lld" + suffix)
        tool.parent.mkdir(parents=True)
        tool.write_bytes(root.name.encode())
        tool.chmod(0o755)
    expected = selected / "lib" / "rustlib" / host / "bin" / ("rust-lld" + suffix)
    search = RustToolSearch(host, selected, compiler)
    path, evidence = search.resolve("rust-lld", cwd=tmp_path, env={"PATH": ""})
    assert path == expected.resolve()
    assert evidence["origin"] == "rust-sysroot-host-tool"
    assert evidence["compiler_host"] == host
    assert evidence["selected_sysroot"] == str(selected)
    # An override sysroot can omit tools: rustc retains its compiler sysroot's
    # host tools as the next authoritative search root.
    empty = tmp_path / "library-only-sysroot"
    empty.mkdir()
    path, _ = RustToolSearch(host, empty, compiler).resolve(
        "rust-lld", cwd=tmp_path, env={"PATH": ""}
    )
    assert (
        path
        == (
            compiler / "lib" / "rustlib" / host / "bin" / ("rust-lld" + suffix)
        ).resolve()
    )


@pytest.mark.parametrize("relative", [False, True])
def test_rust_explicit_linker_path_is_not_replaced_by_bundled_name(tmp_path, relative):
    linker = tmp_path / "explicit" / "rust-lld"
    linker.parent.mkdir()
    linker.write_bytes(b"explicit linker")
    linker.chmod(0o755)
    search = RustToolSearch("x86_64-unknown-linux-gnu", tmp_path, tmp_path)
    value = str(linker.relative_to(tmp_path)) if relative else str(linker)
    path, evidence = search.resolve(value, cwd=tmp_path, env={"PATH": ""})
    assert path == linker.resolve()
    assert evidence["origin"] == "explicit-path"


def _executable(path, content=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content if content is not None else path.name.encode())
    path.chmod(0o755)
    return path


@pytest.mark.parametrize(
    "host,suffix",
    [
        ("x86_64-unknown-linux-gnu", ""),
        ("aarch64-apple-darwin", ""),
        ("x86_64-pc-windows-gnu", ".exe"),
    ],
)
@pytest.mark.parametrize("flavor", ["ld.lld", "ld64.lld", "lld-link", "wasm-ld"])
def test_rust_gcc_ld_wrapper_selects_the_sysroot_rust_lld(
    tmp_path, host, suffix, flavor
):
    # Layout of a rustup toolchain: lib/rustlib/<host>/bin/{rust-lld,gcc-ld/*}.
    bin_dir = tmp_path / "sysroot" / "lib" / "rustlib" / host / "bin"
    wrapper = _executable(bin_dir / "gcc-ld" / (flavor + suffix))
    rust_lld = _executable(bin_dir / ("rust-lld" + suffix))
    search = RustToolSearch(host, tmp_path / "sysroot", tmp_path / "sysroot")

    selected, evidence = search.bundled_lld(wrapper)

    assert selected == rust_lld.resolve()
    assert evidence["origin"] == "rust-lld-wrapper"
    assert evidence["requested"] == str(wrapper)
    assert evidence["content_path"] == str(rust_lld.resolve())


def test_rust_gcc_ld_wrapper_rule_ignores_other_linkers(tmp_path):
    host = "x86_64-unknown-linux-gnu"
    bin_dir = tmp_path / "sysroot" / "lib" / "rustlib" / host / "bin"
    _executable(bin_dir / "rust-lld")
    search = RustToolSearch(host, tmp_path / "sysroot", tmp_path / "sysroot")
    # A system lld, an lld outside any selected sysroot's host tool directory,
    # and an unknown gcc-ld entry are not the Rust wrapper.
    assert search.bundled_lld(_executable(tmp_path / "usr" / "bin" / "ld.lld")) is None
    assert (
        search.bundled_lld(_executable(tmp_path / "other" / "gcc-ld" / "ld.lld"))
        is None
    )
    assert search.bundled_lld(_executable(bin_dir / "gcc-ld" / "ld.gold")) is None
    assert search.bundled_lld(bin_dir / "rust-lld") is None


def test_rust_gcc_ld_entry_linked_to_a_system_lld_is_not_the_wrapper(tmp_path):
    host = "x86_64-unknown-linux-gnu"
    bin_dir = tmp_path / "sysroot" / "lib" / "rustlib" / host / "bin"
    _executable(bin_dir / "rust-lld")
    system_lld = _executable(tmp_path / "usr" / "bin" / "ld.lld")
    entry = bin_dir / "gcc-ld" / "ld.lld"
    entry.parent.mkdir(parents=True)
    try:
        entry.symlink_to(system_lld)
    except OSError as exc:
        pytest.skip(f"executable symlinks unavailable: {exc}")
    search = RustToolSearch(host, tmp_path / "sysroot", tmp_path / "sysroot")
    assert search.bundled_lld(entry) is None


def test_rust_gcc_ld_wrapper_without_rust_lld_fails_closed(tmp_path):
    host = "x86_64-unknown-linux-gnu"
    bin_dir = tmp_path / "sysroot" / "lib" / "rustlib" / host / "bin"
    wrapper = _executable(bin_dir / "gcc-ld" / "ld.lld")
    search = RustToolSearch(host, tmp_path / "sysroot", tmp_path / "sysroot")
    with pytest.raises(ValueError, match="has no rust-lld beside its directory"):
        search.bundled_lld(wrapper)


def test_rust_gcc_ld_wrapper_with_host_dependent_child_fails_closed(tmp_path):
    host = "x86_64-unknown-linux-gnu"
    bin_dir = tmp_path / "sysroot" / "lib" / "rustlib" / host / "bin"
    wrapper = _executable(bin_dir / "gcc-ld" / "ld.lld")
    _executable(bin_dir / "rust-lld")
    # A linked wrapper elsewhere: Linux execs the sysroot rust-lld, macOS and
    # Windows exec the rust-lld beside the invoked spelling.
    elsewhere = tmp_path / "elsewhere" / "bin" / "gcc-ld" / "ld.lld"
    elsewhere.parent.mkdir(parents=True)
    _executable(tmp_path / "elsewhere" / "bin" / "rust-lld", b"another rust-lld")
    try:
        elsewhere.symlink_to(wrapper)
    except OSError as exc:
        pytest.skip(f"executable symlinks unavailable: {exc}")
    search = RustToolSearch(host, tmp_path / "sysroot", tmp_path / "sysroot")
    with pytest.raises(ValueError, match="through its invoked path"):
        search.bundled_lld(elsewhere)


def test_rust_tool_metadata_and_missing_image_fail_closed(tmp_path):
    with pytest.raises(ValueError, match="compiler host"):
        rustc_host("rustc version without host")
    with pytest.raises(ValueError, match="unavailable sysroot"):
        rustc_printed_sysroot("relative/root\n", cwd=tmp_path)
    relative = tmp_path / "relative" / "root"
    relative.mkdir(parents=True)
    assert rustc_printed_sysroot("relative/root\n", cwd=tmp_path) == relative.resolve()
    with pytest.raises(ValueError, match="unique printed sysroot"):
        rustc_printed_sysroot(
            str(tmp_path) + "\nunknown compiler output\n", cwd=tmp_path
        )
    assert rustc_printed_sysroot(str(tmp_path), cwd=tmp_path) == tmp_path.resolve()
    with pytest.raises(ValueError, match="unavailable"):
        RustToolSearch("x86_64-unknown-linux-gnu", tmp_path, tmp_path).resolve(
            "rust-lld", cwd=tmp_path, env={"PATH": ""}
        )


def test_cargo_config_files_use_invocation_cwd_and_stop_at_forwarded_arguments(
    tmp_path,
):
    config = tmp_path / "toolchain.toml"
    config.write_text("[build]\nincremental=false\n", encoding="utf-8")
    assert cargo_config_arguments(
        [
            "cargo",
            "rustc",
            "--config=toolchain.toml",
            "--config",
            "build.incremental=false",
            "--",
            "--config",
            "not-a-cargo-option.toml",
        ],
        cwd=tmp_path,
    ) == ["--config", str(config), "--config", "build.incremental=false"]


def test_rust_relative_tool_path_operands_preserve_bare_linker_search():
    arguments = [
        "--sysroot",
        "root",
        "--sysroot=other",
        "-C",
        "linker=tools/cc",
        "-Clinker=tools/link",
        "-Clinker=rust-lld",
    ]
    assert relative_rustc_tool_paths(arguments) == [
        (1, "", "root"),
        (2, "--sysroot=", "other"),
        (4, "linker=", "tools/cc"),
        (5, "-Clinker=", "tools/link"),
    ]


def _toolchain(tmp_path: Path) -> tuple[Path, Path]:
    rustc = tmp_path / "toolchain" / "bin" / "rustc"
    rustc.parent.mkdir(parents=True)
    rustc.write_text("", encoding="utf-8")
    library = tmp_path / "toolchain" / "lib"
    library.mkdir()
    return rustc, library.resolve()


def test_direct_toolchain_runs_get_rustups_library_path_on_macos(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    rustc, library = _toolchain(tmp_path)
    install_module_view(monkeypatch, "sys", sys, rust_toolchain, platform="darwin")
    env = rust_toolchain.rust_toolchain_library_environment(rustc, {})
    entries = env["DYLD_FALLBACK_LIBRARY_PATH"].split(os.pathsep)
    # The toolchain lib comes first, then dyld's own defaults stay searchable.
    assert entries[0] == str(library)
    assert entries[1:] == [os.path.expanduser("~/lib"), "/usr/local/lib", "/usr/lib"]
    preset = {"DYLD_FALLBACK_LIBRARY_PATH": "/opt/x"}
    assert rust_toolchain.rust_toolchain_library_environment(rustc, preset) == {
        "DYLD_FALLBACK_LIBRARY_PATH": f"{library}{os.pathsep}/opt/x"
    }
    present = {"DYLD_FALLBACK_LIBRARY_PATH": str(library)}
    assert rust_toolchain.rust_toolchain_library_environment(rustc, present) == {}


def test_direct_toolchain_library_path_is_macos_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    rustc, _library = _toolchain(tmp_path)
    install_module_view(monkeypatch, "sys", sys, rust_toolchain, platform="linux")
    assert rust_toolchain.rust_toolchain_library_environment(rustc, {}) == {}


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
def test_codegen_spans_preserve_opaque_values_and_original_path_indexes(spelling):
    value = "linker=tool dir/cc=variant"
    option = (
        (spelling, value)
        if spelling in {"-C", "--codegen"}
        else (("-C" if spelling == "-Cjoined" else spelling) + value,)
    )
    arguments = (
        "--cfg",
        "unrelated",
        *option,
        "-C",
        "link-arg=--codegen=target-feature=-simd128",
        "--",
        "--codegen=linker=positional/file",
    )
    spans = list(rust_toolchain.rust_flag_spans(arguments))
    selected = [span for span in spans if span.codegen is not None]
    assert [(span.start, span.stop, span.codegen) for span in selected] == [
        (2, 2 + len(option), value),
        (
            2 + len(option),
            4 + len(option),
            "link-arg=--codegen=target-feature=-simd128",
        ),
    ]
    expected_index = 1 + len(option)
    expected_prefix = option[-1].removesuffix("tool dir/cc=variant")
    assert relative_rustc_tool_paths(arguments) == [
        (expected_index, expected_prefix, "tool dir/cc=variant")
    ]
    canonical = rust_toolchain.canonical_rust_codegen_flags(arguments)
    assert canonical == (
        "--cfg",
        "unrelated",
        "-C",
        value,
        "-C",
        "link-arg=--codegen=target-feature=-simd128",
        "--",
        "--codegen=linker=positional/file",
    )
    assert rust_toolchain.canonical_rust_codegen_flags(canonical) == canonical


@pytest.mark.parametrize(
    "arguments",
    [
        ("-C",),
        ("--codegen",),
        ("-C", ""),
        ("--codegen=",),
        ("-C=target-feature=+simd128",),
        ("--codegen", "--"),
    ],
)
def test_codegen_admission_rejects_incomplete_and_invalid_spelling(arguments):
    with pytest.raises(ValueError, match="codegen option"):
        rust_toolchain.canonical_rust_codegen_flags(arguments)


@pytest.mark.parametrize(
    "switch",
    [
        "--sysroot",
        "-L",
        "-o",
        "--out-dir",
        "--remap-path-prefix",
        "--remap-path-scope",
        "--cfg",
        "--check-cfg",
        "--extern",
        "--crate-type",
        "--target",
        "--print",
    ],
)
@pytest.mark.parametrize(
    "operand", ["-Clinker=tools/root", "--codegen=linker=tools/root"]
)
def test_rust_outer_operands_are_opaque_to_codegen(switch, operand):
    arguments = (switch, operand, "--codegen", "target-feature=+simd128")
    spans = list(rust_toolchain.rust_flag_spans(arguments))
    assert [(span.start, span.stop, span.codegen) for span in spans] == [
        (0, 2, None),
        (2, 4, "target-feature=+simd128"),
    ]
    assert rust_toolchain.canonical_rust_codegen_flags(arguments) == (
        switch,
        operand,
        "-C",
        "target-feature=+simd128",
    )
    assert relative_rustc_tool_paths(arguments) == (
        [(1, "", operand)] if switch == "--sysroot" else []
    )


@pytest.mark.parametrize("output", ["-C", "-Lnative=opaque", "--codegen"])
def test_codegen_projection_does_not_rescan_output_operand(output):
    arguments = ("-o", output, "--codegen=linker=tools/real")
    canonical = rust_toolchain.canonical_rust_codegen_flags(arguments)
    assert canonical == ("-o", output, "-C", "linker=tools/real")
    assert rust_toolchain.canonical_rust_codegen_flags(canonical) == canonical
    assert relative_rustc_tool_paths(arguments) == [
        (2, "--codegen=linker=", "tools/real")
    ]


@pytest.mark.parametrize(
    "arguments,canonical,index,prefix",
    [
        (
            ("-gClinker=tools/a_b=cc",),
            ("-g", "-C", "linker=tools/a_b=cc"),
            0,
            "-gClinker=",
        ),
        (
            ("-vC", "linker=tools/a_b=cc"),
            ("-v", "-C", "linker=tools/a_b=cc"),
            1,
            "linker=",
        ),
        (
            ("-gvOC", "linker=tools/a_b=cc"),
            ("-gvO", "-C", "linker=tools/a_b=cc"),
            1,
            "linker=",
        ),
    ],
)
def test_short_codegen_clusters_preserve_prefix_and_original_tool_span(
    arguments, canonical, index, prefix
):
    assert rust_toolchain.canonical_rust_codegen_flags(arguments) == canonical
    assert relative_rustc_tool_paths(arguments) == [(index, prefix, "tools/a_b=cc")]
    assert rust_toolchain.canonical_rust_codegen_flags(canonical) == canonical


@pytest.mark.parametrize(
    "cluster", ["-gL", "-vl", "-Oo", "-gZ", "-vW", "-gA", "-gD", "-gF"]
)
def test_short_value_option_consumes_remainder_or_next_opaque_token(cluster):
    for arguments in (
        (cluster, "--codegen=linker=opaque"),
        (cluster + "-Clinker=opaque",),
    ):
        (span,) = rust_toolchain.rust_flag_spans(arguments)
        assert span.leading == (cluster[:-1],)
        assert span.option == "-" + cluster[-1]
        assert span.value in {"--codegen=linker=opaque", "-Clinker=opaque"}
        assert span.codegen is None
        assert rust_toolchain.canonical_rust_codegen_flags(arguments) == arguments


@pytest.mark.parametrize(
    "value,expected",
    [
        ("link_arg=--path=with_under_score", "link-arg=--path=with_under_score"),
        (
            "target_feature=+simd128,-reference-types",
            "target-feature=+simd128,-reference-types",
        ),
        ("link_self_contained=no", "link-self-contained=no"),
        ("linker_flavor=wasm-ld", "linker-flavor=wasm-ld"),
        ("link_args=opaque_1 opaque_2", "link-args=opaque_1 opaque_2"),
    ],
)
def test_codegen_key_alias_normalization_preserves_value_bytes(value, expected):
    arguments = ("-gC" + value,)
    (span,) = rust_toolchain.rust_flag_spans(arguments)
    assert span.value == value and span.codegen == expected
    assert rust_toolchain.canonical_rust_codegen_flags(arguments) == (
        "-g",
        "-C",
        expected,
    )

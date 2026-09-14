import os

import pytest

from molt.rust_toolchain import (
    RustToolSearch,
    cargo_config_arguments,
    cargo_configuration_paths,
    relative_rustc_tool_paths,
    rustc_host,
    rustc_printed_sysroot,
)


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

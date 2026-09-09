from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli.cargo_target_cfg import (
    cargo_target_query_arguments,
    parse_cargo_cfg,
    parse_rustc_cfg,
    parse_rustc_target_metadata,
    select_cargo_target_flags,
)


@pytest.mark.parametrize("target", [None, "wasm32-wasip1"])
def test_metadata_query_matches_cargo_wrapper_visible_envelope(
    target: str | None,
) -> None:
    args = cargo_target_query_arguments(target, ("--cfg", "selected"))
    assert args == (
        "-",
        "--crate-name",
        "___",
        "--print=file-names",
        "--cfg",
        "selected",
        *(("--target", target) if target is not None else ()),
        "--crate-type",
        "bin",
        "--crate-type",
        "rlib",
        "--crate-type",
        "dylib",
        "--crate-type",
        "cdylib",
        "--crate-type",
        "staticlib",
        "--crate-type",
        "proc-macro",
        "--print=sysroot",
        "--print=split-debuginfo",
        "--print=crate-name",
        "--print=cfg",
        "-Wwarnings",
    )


@pytest.mark.parametrize("unsupported", [False, True])
def test_target_metadata_uses_cargo_unsupported_crate_and_delimiter_protocol(
    tmp_path: Path, unsupported: bool
) -> None:
    kinds = ("bin", "rlib", "dylib", "cdylib", "staticlib", "proc-macro")
    stdout = "\n".join(
        (
            *(f"___{kind}" for kind in kinds if not unsupported or kind != "dylib"),
            str(tmp_path),
            "off",
            "packed",
            "___",
            "unix",
            "proc_macro",
            'target_arch="x86_64"',
        )
    )
    stderr = (
        "warning: dropping unsupported crate type `dylib` for target"
        if unsupported
        else ""
    )
    metadata = parse_rustc_target_metadata(stdout, stderr)
    assert metadata.sysroot == tmp_path
    assert (
        metadata.target_libdir("target")
        == tmp_path / "lib" / "rustlib" / "target" / "lib"
    )
    assert metadata.split_debuginfo == ("off", "packed")
    assert ("proc_macro", None) not in metadata.cfg
    assert dict(metadata.crate_filenames)["dylib"] == (
        None if unsupported else "___dylib"
    )


@pytest.mark.parametrize("missing", ["filename", "sysroot", "delimiter", "cfg"])
def test_incomplete_target_metadata_cannot_authorize_cfg_or_resources(
    tmp_path: Path, missing: str
) -> None:
    lines = ["___"] * 6 + [str(tmp_path), "off", "___", "unix"]
    if missing == "filename":
        lines[0] = "not-a-crate-filename"
    elif missing == "sysroot":
        lines[6] = "relative-root"
    elif missing == "delimiter":
        lines[8] = "no-delimiter"
    else:
        lines.pop()
    with pytest.raises(ValueError):
        parse_rustc_target_metadata("\n".join(lines), "")


@pytest.mark.parametrize(
    ("predicate", "expected"),
    [
        ('cfg(target_arch = "wasm32")', True),
        ('cfg(not(target_arch = "wasm32"))', False),
        ('cfg(all(target_arch = "wasm32", target_os = "wasi",))', True),
        ("cfg(any(unix, windows))", False),
        ("cfg(all())", True),
        ("cfg(any())", False),
        ('cfg(target_feature = "mutable-globals")', True),
    ],
)
def test_predicates_use_reported_target_facts(predicate: str, expected: bool) -> None:
    facts = parse_rustc_cfg(
        'target_arch="wasm32"\ntarget_os="wasi"\n'
        'target_feature="mutable-globals"\ntarget_feature="sign-ext"\n'
    )
    assert parse_cargo_cfg(predicate).matches(facts) is expected


@pytest.mark.parametrize(
    "text",
    [
        "cfg()",
        "cfg(not())",
        "cfg(not(unix,windows))",
        "cfg(foo())",
        "cfg(target_os=linux)",
        "cfg(unix) trailing",
        "cfg(all(unix windows))",
        "cfg(any(unix)",
        "cfg(unix))",
    ],
)
def test_invalid_predicates_fail_closed(text: str) -> None:
    with pytest.raises(ValueError):
        parse_cargo_cfg(text)


def test_cargo_cfg_omits_proc_macro_without_dropping_target_facts() -> None:
    assert parse_rustc_cfg('proc_macro\nunix\nproc_macro="value"') == frozenset(
        {("unix", None), ("proc_macro", "value")}
    )


def test_flag_precedence_converges_and_retains_target_cfg_order() -> None:
    probes: list[tuple[str, ...]] = []

    def probe(flags: tuple[str, ...]):
        probes.append(flags)
        return parse_rustc_cfg('unix\ntarget_arch="x86_64"')

    selected, matches = select_cargo_target_flags(
        {
            "cfg(unix)": {"rustflags": ["--cfg", "unix_flag"]},
            'cfg(target_arch="x86_64")': {"rustflags": ["--cfg", "arch_flag"]},
            "cfg(windows)": {"linker": "wrong"},
        },
        target_flags=("--cfg", "exact"),
        build_flags=("--cfg", "build"),
        environment_flags=None,
        flags=tuple,
        probe=probe,
    )
    assert selected == ("--cfg", "exact", "--cfg", "arch_flag", "--cfg", "unix_flag")
    assert probes == [("--cfg", "exact"), selected]
    assert [key for key, _ in matches] == ['cfg(target_arch="x86_64")', "cfg(unix)"]


@pytest.mark.parametrize("override", [(), ("--cfg", "environment")])
def test_environment_flags_override_all_target_and_build_flags(override) -> None:
    selected, _ = select_cargo_target_flags(
        {"cfg(unix)": {"rustflags": ["--cfg", "ignored"]}},
        target_flags=("--cfg", "ignored_exact"),
        build_flags=("--cfg", "ignored_build"),
        environment_flags=override,
        flags=tuple,
        probe=lambda _: parse_rustc_cfg("unix"),
    )
    assert selected == override


def test_empty_target_flags_use_build_flags_and_inactive_cfg_is_not_an_error() -> None:
    selected, matches = select_cargo_target_flags(
        {'cfg(not(target_arch="wasm32"))': {"rustflags": []}},
        target_flags=(),
        build_flags=("--cfg", "build"),
        environment_flags=None,
        flags=tuple,
        probe=lambda _: parse_rustc_cfg('target_arch="wasm32"'),
    )
    assert selected == ("--cfg", "build")
    assert matches == ()


def test_cfg_flag_cycle_cannot_receive_a_runtime_identity() -> None:
    with pytest.raises(ValueError, match="does not converge"):
        select_cargo_target_flags(
            {"cfg(not(flip))": {"rustflags": ["--cfg", "flip"]}},
            target_flags=(),
            build_flags=(),
            environment_flags=None,
            flags=tuple,
            probe=lambda flags: parse_rustc_cfg("unix\nflip" if flags else "unix"),
        )


def test_final_transform_is_pinned_once_and_reselects_cfg_linker() -> None:
    transforms = []

    def transform(flags):
        transforms.append(flags)
        return (*flags, "--cfg", "selected")

    selected, matches = select_cargo_target_flags(
        {"cfg(selected)": {"linker": "selected-linker", "rustflags": ["ignored"]}},
        target_flags=(),
        build_flags=(),
        environment_flags=None,
        flags=tuple,
        probe=lambda flags: parse_rustc_cfg("unix\nselected" if flags else "unix"),
        transform=transform,
    )
    assert transforms == [()]
    assert selected == ("--cfg", "selected")
    assert matches == (
        ("cfg(selected)", {"linker": "selected-linker", "rustflags": ["ignored"]}),
    )

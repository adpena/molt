from __future__ import annotations

import inspect
import json
import subprocess
import sys
from pathlib import Path
from typing import Any, cast

import pytest

from molt.backend_environment import CodegenSelection
import molt.cli as cli
from molt.cli import build_inputs as cli_build_inputs
from molt.cli import script_commands as cli_commands
from molt.cli import runtime_wasm_build_policy as cli_runtime_build
from molt.cli import wrapper_build


def test_prepare_build_config_uses_dev_runtime_profile_for_dev_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli_build_inputs._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        target="native",
        profile="dev",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
        codegen=CodegenSelection(),
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "dev-fast"


def test_prepare_build_config_uses_release_runtime_profile_for_release_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli_build_inputs._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        target="native",
        profile="release",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
        codegen=CodegenSelection(),
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "release-output"


@pytest.mark.parametrize(
    ("target", "profile", "requested", "resolved"),
    [
        ("native", "release", "release-output", "release-output"),
        ("llvm", "release", "release-output", "release-output"),
        ("wasm", "release", "release", "wasm-release"),
        ("wasm-freestanding", "release", "release", "wasm-release"),
        ("native", "dev", "dev-fast", "dev-fast"),
        ("wasm", "dev", "dev-fast", "dev-fast"),
        ("wasm-freestanding", "dev", "dev-fast", "dev-fast"),
    ],
)
def test_public_runtime_profile_request_preserves_target_policy(
    tmp_path, monkeypatch, target, profile, requested, resolved
):
    for name in (
        "MOLT_DEV_CARGO_PROFILE",
        "MOLT_RELEASE_CARGO_PROFILE",
        "MOLT_RUNTIME_BUILD_PROFILE",
        "MOLT_WASM_CARGO_PROFILE",
    ):
        monkeypatch.delenv(name, raising=False)
    prepared, error = cli_build_inputs._prepare_build_config(
        project_root=tmp_path,
        codegen=CodegenSelection(),
        warnings=[],
        json_output=False,
        target=target,
        profile=profile,
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )
    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == requested
    actual = (
        cli_runtime_build._resolve_wasm_cargo_profile(prepared.runtime_cargo_profile)
        if target in {"wasm", "wasm-freestanding"}
        else prepared.runtime_cargo_profile
    )
    assert actual == resolved


@pytest.mark.parametrize("target", ["native", "wasm", "wasm-freestanding"])
@pytest.mark.parametrize(
    ("explicit", "iteration", "wasm_override", "wasm_expected"),
    [
        ("release-output", None, None, "release-output"),
        ("wasm-release", None, None, "wasm-release"),
        ("custom-release", None, None, "custom-release"),
        ("release-output", "dev-fast", None, "dev-fast"),
        ("release-output", "dev-fast", "wasm-release", "wasm-release"),
        ("wasm-release", None, "release-output", "release-output"),
    ],
)
def test_explicit_runtime_profile_survives_public_selection(
    tmp_path, monkeypatch, target, explicit, iteration, wasm_override, wasm_expected
):
    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", explicit)
    for name, value in (
        ("MOLT_RUNTIME_BUILD_PROFILE", iteration),
        ("MOLT_WASM_CARGO_PROFILE", wasm_override),
    ):
        if value is None:
            monkeypatch.delenv(name, raising=False)
        else:
            monkeypatch.setenv(name, value)
    prepared, error = cli_build_inputs._prepare_build_config(
        project_root=tmp_path,
        codegen=CodegenSelection(),
        warnings=[],
        json_output=False,
        target=target,
        profile="release",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )
    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == explicit
    if target in {"wasm", "wasm-freestanding"}:
        assert cli_runtime_build._resolve_wasm_cargo_profile(explicit) == wasm_expected
    else:
        assert prepared.runtime_cargo_profile == explicit


@pytest.mark.parametrize("target", ["native", "wasm", "wasm-freestanding"])
def test_invalid_public_runtime_profile_is_rejected_for_every_target(
    tmp_path, monkeypatch, capsys, target
):
    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", "bad profile")
    prepared, error = cli_build_inputs._prepare_build_config(
        project_root=tmp_path,
        codegen=CodegenSelection(),
        warnings=[],
        json_output=False,
        target=target,
        profile="release",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )
    assert prepared is None
    assert error == 2
    assert (
        capsys.readouterr().err.strip()
        == "Invalid MOLT_RELEASE_CARGO_PROFILE value: bad profile"
    )


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_build_profile_flag_routes_to_build_profile(
    profile: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    seen_profiles: list[str] = []
    build_signature = inspect.signature(cli.build)

    def fake_build(*args: Any, **kwargs: Any) -> int:
        bound = build_signature.bind_partial(*args, **kwargs)
        seen_profiles.append(cast(str, bound.arguments["profile"]))
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        ["molt", "build", "--profile", profile, str(entry)],
    )

    assert cli.main() == 0
    assert seen_profiles == [profile]


def test_build_args_profile_detection_keeps_platform_profile_separate() -> None:
    assert not cli._build_args_has_profile_flag(["--profile", "browser"])
    assert not cli._build_args_has_profile_flag(["--profile=browser"])
    assert cli._build_args_has_profile_flag(["--profile", "dev"])
    assert cli._build_args_has_profile_flag(["--profile=release"])
    assert cli._build_args_has_profile_flag(["--build-profile", "dev"])
    assert cli._build_args_has_profile_flag(
        ["--profile", "browser", "--build-profile", "dev"]
    )


@pytest.mark.parametrize("build_profile", ["dev", "release"])
@pytest.mark.parametrize("platform_profile", [None, "browser"])
def test_nested_build_keeps_platform_profile_and_forwards_build_profile(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    build_profile: str,
    platform_profile: str | None,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    output_binary = tmp_path / "bin" / "main_molt"
    output_binary.parent.mkdir(parents=True)
    output_binary.write_text("", encoding="utf-8")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(output_binary),
            "consumer_output": str(output_binary),
        },
    )
    build_cmds: list[list[str]] = []
    run_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli_commands, "compiler_source_root", lambda: project)
    monkeypatch.setattr(wrapper_build, "_run_completed_command", fake_subprocess_run)
    # Profile forwarding owns neither import closure computation nor cache
    # publication. Exercise an uncacheable entry; the real closure/cache
    # contracts live in test_cli_binary_image_closure and import-collection tests.
    monkeypatch.setattr(wrapper_build, "_wrapper_build_cache_input", lambda **_: None)

    def fake_run_command(cmd: list[str], **_kwargs: object) -> int:
        run_cmds.append(list(cmd))
        return 0

    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command)
    platform_args = ["--profile", platform_profile] if platform_profile else []

    rc = cli_commands.run_script(
        str(entry),
        None,
        [],
        build_args=platform_args,
        build_profile=build_profile,
        json_output=False,
    )

    assert rc == 0
    assert build_cmds[-1:] == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
            *platform_args,
            "--build-profile",
            build_profile,
            str(entry),
        ]
    ]
    assert run_cmds == [[str(output_binary)]]


# ---------------------------------------------------------------------------
# Runtime-wasm iteration knobs (Hotspot 1/2): dev-fast profile + incremental
# target dir. Overrides are explicit; public WASM release resolves to wasm-release.
# ---------------------------------------------------------------------------


def _clear_wasm_profile_cache() -> None:
    cli_runtime_build._resolve_wasm_cargo_profile_cached.cache_clear()


def test_runtime_build_profile_preserves_explicit_release_output(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_BUILD_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_WASM_CARGO_PROFILE", raising=False)
    _clear_wasm_profile_cache()
    # An explicit physical profile remains available independently of defaults.
    assert cli_runtime_build._resolve_wasm_cargo_profile("release-output") == (
        "release-output"
    )
    # generic "release" still maps to wasm-release
    _clear_wasm_profile_cache()
    assert cli_runtime_build._resolve_wasm_cargo_profile("release") == "wasm-release"


def test_runtime_build_profile_knob_overrides_wasm_profile(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "dev-fast")
    monkeypatch.delenv("MOLT_WASM_CARGO_PROFILE", raising=False)
    _clear_wasm_profile_cache()
    assert cli_runtime_build._resolve_wasm_cargo_profile("release-output") == "dev-fast"
    _clear_wasm_profile_cache()


def test_explicit_wasm_cargo_profile_wins_over_runtime_build_profile(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "dev-fast")
    monkeypatch.setenv("MOLT_WASM_CARGO_PROFILE", "wasm-release")
    _clear_wasm_profile_cache()
    assert cli_runtime_build._resolve_wasm_cargo_profile("release-output") == (
        "wasm-release"
    )
    _clear_wasm_profile_cache()


def test_runtime_build_profile_ignores_invalid_name(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "bad name; rm -rf")
    monkeypatch.delenv("MOLT_WASM_CARGO_PROFILE", raising=False)
    _clear_wasm_profile_cache()
    assert cli_runtime_build._resolve_wasm_cargo_profile("release-output") == (
        "release-output"
    )
    _clear_wasm_profile_cache()


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("", False),
        ("0", False),
        ("off", False),
        ("1", True),
        ("true", True),
        ("YES", True),
        ("On", True),
    ],
)
def test_runtime_wasm_incremental_enabled_env(
    value: str,
    expected: bool,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if value:
        monkeypatch.setenv("MOLT_RUNTIME_WASM_INCREMENTAL", value)
    else:
        monkeypatch.delenv("MOLT_RUNTIME_WASM_INCREMENTAL", raising=False)
    assert cli_runtime_build._runtime_wasm_incremental_enabled() is expected


def test_incremental_family_key_excludes_link_args_and_separates_configs() -> None:
    # The family key is the codegen identity: reloc vs shared differ only by
    # link-args (not an argument here), so both passes map to ONE dir/key.
    base = dict(
        cargo_profile="release-output",
        target_triple="wasm32-wasip1",
        features=("stdlib_math", "stdlib_regex"),
        simd_enabled=True,
        freestanding=False,
    )
    key = cli_runtime_build._runtime_wasm_incremental_family_key(**base)
    # Stable across feature ordering (sorted internally).
    reordered = dict(base, features=("stdlib_regex", "stdlib_math"))
    assert cli_runtime_build._runtime_wasm_incremental_family_key(**reordered) == key
    # Genuinely different configs get different families.
    assert (
        cli_runtime_build._runtime_wasm_incremental_family_key(
            **dict(base, cargo_profile="dev-fast")
        )
        != key
    )
    assert (
        cli_runtime_build._runtime_wasm_incremental_family_key(
            **dict(base, simd_enabled=False)
        )
        != key
    )
    assert (
        cli_runtime_build._runtime_wasm_incremental_family_key(
            **dict(base, freestanding=True)
        )
        != key
    )
    assert (
        cli_runtime_build._runtime_wasm_incremental_family_key(
            **dict(base, features=("stdlib_math",))
        )
        != key
    )
    # Readable profile prefix.
    assert key.startswith("release-output-")


def test_incremental_target_root_is_session_independent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.setenv("MOLT_SESSION_ID", "session-abc")
    root = cli_runtime_build._runtime_wasm_incremental_target_root(
        tmp_path, "release-output-deadbeef"
    )
    # No per-session component: the whole point is cross-iteration reuse.
    assert "sessions" not in root.parts
    assert root == tmp_path / "target" / "runtime-wasm-incr" / "release-output-deadbeef"


def test_incremental_target_root_honors_cargo_target_dir(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    override = tmp_path / "custom-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(override))
    root = cli_runtime_build._runtime_wasm_incremental_target_root(
        tmp_path, "dev-fast-cafef00d"
    )
    assert root == override / "runtime-wasm-incr" / "dev-fast-cafef00d"

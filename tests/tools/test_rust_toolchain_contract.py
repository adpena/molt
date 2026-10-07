from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECK_RUST_TOOLCHAIN = ROOT / "tools" / "check_rust_toolchain.py"


def _load_check_rust_toolchain():
    spec = importlib.util.spec_from_file_location(
        "molt_test_check_rust_toolchain",
        CHECK_RUST_TOOLCHAIN,
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_repository_rust_toolchain_contract_is_canonical() -> None:
    tool = _load_check_rust_toolchain()

    report = tool.check_repository_contract()

    assert report.errors == ()


def test_ci_gate_uses_repo_rust_toolchain_gate_without_compile_slot() -> None:
    spec = importlib.util.spec_from_file_location(
        "molt_test_ci_gate_for_rust_toolchain",
        ROOT / "tools" / "ci_gate.py",
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)

    check = {entry.name: entry for entry in module._build_checks()}["rust-toolchain"]

    assert check.cmd == [
        sys.executable,
        str(module.TOOLS / "check_rust_toolchain.py"),
    ]
    assert check.needs_rust is False
    assert check.needs_cargo is True


def test_cargo_version_probe_normalizes_config_wrapper(monkeypatch) -> None:
    tool = _load_check_rust_toolchain()
    captured: dict[str, object] = {}

    def fake_run(_command: list[str], **kwargs: object):
        captured.update(kwargs)
        return subprocess.CompletedProcess([], 0, "cargo 1.96.1", "")

    monkeypatch.setenv("CARGO_BUILD_RUSTC_WRAPPER", "sccache")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    monkeypatch.setattr(tool.subprocess, "run", fake_run)

    tool._run(["cargo", "--version"])

    assert captured["env"]["CARGO_INCREMENTAL"] == "0"  # type: ignore[index]


def test_selected_compiler_meets_workspace_minimum() -> None:
    tool = _load_check_rust_toolchain()
    pinned = tool.RUST_VERSION
    major, minor, _patch = (int(part) for part in pinned.split("."))
    assert not tool.check_compiler_version(
        f"rustc {major}.{minor - 1}.0 (2d76d9bc7 2026-03-09)"
    ).ok
    assert not tool.check_compiler_version(
        f"rustc {pinned}-nightly (abcdef 2026-06-01)"
    ).ok
    assert tool.check_compiler_version(f"rustc {pinned} (31fca3adb 2026-06-26)").ok
    assert tool.check_compiler_version(
        f"rustc {major}.{minor + 2}.0-nightly (c1070d693 2026-09-28)"
    ).ok
    assert not tool.check_compiler_version("garbage").ok


def test_pinned_version_is_the_rust_toolchain_channel() -> None:
    import tomllib

    tool = _load_check_rust_toolchain()
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text("utf-8"))

    assert tool.RUST_VERSION == toolchain["toolchain"]["channel"]


def test_workflows_name_rust_toolchain_roles_not_versions() -> None:
    tool = _load_check_rust_toolchain()
    workflow = Path(".github/workflows/fixture.yml")
    text = (
        "jobs:\n"
        "  a:\n"
        "    steps:\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: pinned\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: sanitizer-nightly\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        f'          rust-toolchain: "{tool.RUST_VERSION}"\n'
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: stable\n"
    )

    errors = tool.workflow_rust_toolchain_errors(workflow, text)

    assert [error.split(":", 2)[1] for error in errors] == ["12", "15"]
    assert all("must name a role" in error for error in errors)

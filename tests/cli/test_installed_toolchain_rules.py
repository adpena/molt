"""An installed Molt never requires, probes or installs Rust, and never adopts a
nearby source checkout; source development tooling stays explicit."""

from __future__ import annotations

# Shared pytest fixtures are imported by name and requested as parameters.
# ruff: noqa: F401, F811

import hashlib
import json
from pathlib import Path
import sysconfig

import pytest

from molt import compiler_distribution as distribution
from molt import source_root
from molt.cli import compiler_metadata, project_roots, setup_readiness
from molt.cli import toolchain_validation, wasm_toolchain
from molt.cli.installation_diagnostics import installation_checks
from tests.cli.test_installed_runtime import bundle as bundle

# These cases build synthetic projects and assert developer-host roots;
# hosted custody has its own cases in tests/test_dx_run_context.py.
pytestmark = pytest.mark.usefixtures("developer_host_context")

_RUST_COMMANDS = {"cargo", "rustc", "rustup", "cargo-upgrade", "sccache"}
_RUST_CHECKS = {
    "cargo",
    "rust-toolchain",
    "rust-components",
    "rustup",
    "cargo-upgrade",
    "sccache",
    "cargo-target-dir",
    "molt-diff-target-dir",
    "wasm-target",
}


def _complete_toolchain_inputs(bundle: Path) -> None:
    """Give the transport fixture real contracts consumed by doctor/update."""
    source = bundle / "source"
    manifest = source / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    records = {entry["path"]: entry for entry in payload["files"]}
    for name in ("pyproject.toml", "config/tool_releases.toml"):
        data = (Path(__file__).resolve().parents[2] / name).read_bytes()
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        records[name] = {
            "path": name,
            "mode": 0o100644,
            "blob_oid": "c" * 40,
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    payload["files"] = [records[name] for name in sorted(records)]
    manifest.write_text(json.dumps(payload), encoding="utf-8")


def _refuse_rust(monkeypatch) -> None:
    real_which = setup_readiness.shutil.which

    def which(name, *args, **kwargs):
        if Path(str(name)).name in _RUST_COMMANDS:
            pytest.fail(f"installed readiness probed {name}")
        return real_which(name, *args, **kwargs)

    def refuse(*_args, **_kwargs):
        pytest.fail("installed readiness consulted the Rust toolchain contract")

    monkeypatch.setattr(setup_readiness.shutil, "which", which)
    monkeypatch.setattr(wasm_toolchain, "rust_toolchain_contract", refuse)
    monkeypatch.setattr(wasm_toolchain, "rust_target_readiness_error", refuse)


def test_installed_doctor_reports_shipped_cells_without_rust(bundle, monkeypatch):
    _complete_toolchain_inputs(bundle)
    _refuse_rust(monkeypatch)
    report = setup_readiness._build_toolchain_report(bundle / "source")
    names = {check["name"] for check in report.checks}
    assert not names & _RUST_CHECKS
    assert not any(
        name in {"installation-cargo", "installation-rustc"} for name in names
    )
    runtime = next(check for check in report.checks if check["name"] == "molt-runtime")
    assert runtime["ok"], runtime
    assert report.backends["llvm"] is False
    # The fixture ships one dev-profile native cell and no WASM cells.
    assert report.profiles == {"dev": True, "release": False}
    assert report.backends["wasm"] is False


def test_damaged_installed_metadata_is_installed_damage_not_source_mode(
    bundle,
    monkeypatch,
):
    _complete_toolchain_inputs(bundle)
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    del payload["runtime"]
    manifest.write_text(json.dumps(payload), "utf-8")
    checks = installation_checks(bundle / "source")
    assert checks[0]["ok"] is False and checks[0]["level"] == "error"
    assert "damaged installation" in checks[0]["detail"]
    assert not any(check["name"] == "installation-cargo" for check in checks)
    _refuse_rust(monkeypatch)
    report = setup_readiness._build_toolchain_report(bundle / "source")
    names = {check["name"] for check in report.checks}
    assert "molt-installation-integrity" in names and not names & _RUST_CHECKS


def test_missing_manifest_in_a_packaged_distribution_is_damage(tmp_path, monkeypatch):
    packaged = tmp_path / "prefix" / "share" / "molt" / "distribution"
    (packaged / "source").mkdir(parents=True)
    monkeypatch.delenv("MOLT_BUNDLE_ROOT", raising=False)
    monkeypatch.setattr(distribution, "packaged_distribution_root", lambda: packaged)
    with pytest.raises(ValueError, match="manifest is missing"):
        distribution.installed_compiler(packaged / "source")
    assert distribution.installed_compiler(tmp_path) is None


def test_installed_update_plans_no_rust_toolchain_steps(bundle, monkeypatch):
    _complete_toolchain_inputs(bundle)
    real_which = toolchain_validation.shutil.which

    def which(name, *args, **kwargs):
        if name in _RUST_COMMANDS:
            pytest.fail(f"installed update probed {name}")
        return real_which(name, *args, **kwargs)

    monkeypatch.setattr(toolchain_validation.shutil, "which", which)
    steps, warnings = toolchain_validation._planned_update_steps(
        bundle / "source",
        include_toolchains=True,
        include_locks=False,
        include_manifests=False,
        source_checkout=False,
    )
    assert not any(
        "rustup" in " ".join(step.cmd) or step.cmd[:1] == ["cargo"] for step in steps
    )
    assert any("does not use or update Rust" in warning for warning in warnings)


def test_installed_compiler_cache_keys_never_probe_host_rustc(bundle, monkeypatch):
    monkeypatch.setattr(compiler_metadata, "_compiler_root", lambda: bundle / "source")
    monkeypatch.setattr(
        compiler_metadata,
        "_run_completed_command",
        lambda *a, **k: pytest.fail("installed metadata ran rustc"),
    )
    assert compiler_metadata._rustc_version.__wrapped__() is None


def test_installed_native_symbol_consumers_never_discover_rust(bundle, monkeypatch):
    from molt.cli import llvm_wasi_tools, native_symbol_inspection

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(bundle / "source"))

    def refuse(**_kwargs):
        pytest.fail("installed native object inspection discovered a Rust toolchain")

    monkeypatch.setattr(llvm_wasi_tools, "_rust_llvm_bin_directories", refuse)
    # This is the shared path for app cache objects and source extensions too;
    # installed runtime codegen uses its shipped projection instead.
    native_symbol_inspection._nm_candidate_binaries()


def test_packaged_distribution_is_found_only_in_the_owning_install_scheme(
    tmp_path, monkeypatch
):
    package_parent = str(Path(source_root.__file__).resolve().parent.parent)
    data = tmp_path / "prefix"
    other = tmp_path / "other"
    (data / "share" / "molt" / "distribution").mkdir(parents=True)
    (other / "share" / "molt" / "distribution").mkdir(parents=True)
    schemes = {
        "foreign": {"purelib": str(tmp_path / "lib"), "data": str(other)},
        "owner": {
            "purelib": package_parent,
            "platlib": package_parent,
            "data": str(data),
        },
    }
    monkeypatch.setattr(sysconfig, "get_scheme_names", lambda: tuple(schemes))
    monkeypatch.setattr(sysconfig, "get_paths", lambda scheme: schemes[scheme])
    assert source_root.packaged_distribution_root() == (
        data / "share" / "molt" / "distribution"
    )
    monkeypatch.delenv("MOLT_SOURCE_ROOT", raising=False)
    assert source_root.compiler_source_root() == (
        data / "share" / "molt" / "distribution" / "source"
    )
    del schemes["owner"]
    assert source_root.packaged_distribution_root() is None


@pytest.mark.parametrize("packaged_install", [False, True])
def test_installed_cli_never_adopts_a_nearby_checkout(
    tmp_path, monkeypatch, packaged_install
):
    checkout = tmp_path / "checkout"
    packaged = tmp_path / "distribution"
    for root in (checkout, packaged / "source"):
        for marker in (
            "Cargo.toml",
            "runtime/molt-runtime/Cargo.toml",
            "runtime/molt-backend/Cargo.toml",
            "src/molt/cli/__init__.py",
        ):
            path = root / marker
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"")
    monkeypatch.delenv("MOLT_SOURCE_ROOT", raising=False)
    monkeypatch.setattr(
        source_root, "_DEFAULT_COMPILER_SOURCE_ROOT", packaged / "source"
    )
    monkeypatch.setattr(
        source_root,
        "packaged_distribution_root",
        lambda: packaged if packaged_install else None,
    )
    monkeypatch.chdir(checkout)
    assert source_root.compiler_source_root() == packaged / "source"
    # Explicit source development selection is live, never a cached cwd choice.
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(checkout))
    assert source_root.compiler_source_root() == checkout
    monkeypatch.delenv("MOLT_SOURCE_ROOT")
    assert source_root.compiler_source_root() == packaged / "source"


def _installed_with_package(tmp_path: Path, files: dict[str, bytes]):
    inventory = tuple(
        {
            "path": f"src/molt/{name}",
            "mode": 0o100644,
            "blob_oid": "c" * 40,
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        for name, data in sorted(files.items())
    )
    return distribution.InstalledCompiler(
        tmp_path / "prefix" / "share" / "molt" / "distribution" / "source",
        "a" * 40,
        {},
        {},
        inventory,
        {},
    )


@pytest.mark.parametrize("damage", [None, "changed", "extra", "cache"])
def test_pip_executing_package_is_bound_to_the_signed_source(
    tmp_path, monkeypatch, damage
):
    package = tmp_path / "site-packages" / "molt"
    (package / "cli").mkdir(parents=True)
    files = {"__init__.py": b"", "cli/__init__.py": b"main = None\n"}
    for name, data in files.items():
        (package / name).write_bytes(data)
    monkeypatch.setattr(
        distribution, "__file__", str(package / "compiler_distribution.py")
    )
    files["compiler_distribution.py"] = b"# placeholder\n"
    (package / "compiler_distribution.py").write_bytes(
        files["compiler_distribution.py"]
    )
    installed = _installed_with_package(tmp_path, files)
    if damage == "changed":
        (package / "cli/__init__.py").write_bytes(b"main = 'patched'\n")
    elif damage == "extra":
        (package / "injected.py").write_bytes(b"")
    elif damage == "cache":
        (package / "__pycache__").mkdir()
        (package / "__pycache__" / "x.cpython-312.pyc").write_bytes(b"cache")
    if damage in {"changed", "extra"}:
        with pytest.raises(ValueError, match="Installed Molt package"):
            installed.verify_executing_package()
    else:
        installed.verify_executing_package()

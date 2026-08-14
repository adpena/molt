from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

import molt.cli.source_extension_set_registry as set_registry
from molt.cli.source_extension_set_registry import (
    source_extension_set_expected_identity,
    verify_source_extension_abi_headers,
)
from molt.scientific_stack_versions import (
    CONFIG_ENV,
    attest_numpy_witness_seal,
    resolve_scientific_stack,
    scientific_custody_root,
    scientific_witness_seal_root,
    scientific_witness_variant,
)

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"


def _write_configs(
    root: Path,
    *,
    selected_numpy: str = "2.5.1",
    selected_scipy: str = "1.18.0",
    selected_cpython: str = "3.12",
    verified_numpy: str = "2.5.1",
    verified_scipy: str = "1.18.0",
    verified_cpython: str = "3.12",
    scientific_schema: int = 6,
) -> Path:
    registry = root / "registry.toml"
    registry.write_text(
        f'''schema_version = 1

[[packages]]
name = "numpy"
version = "{verified_numpy}"
[packages.source]
kind = "git"
commit = "{"a" * 40}"
[[packages.sets]]
name = "pact-witness"
seal_name = "numpy-witness"
build_dependency_group = "source-build-numpy"
use_pkg_config = false
required_config_tools = []
required_installed_files = ["numpy/__init__.py"]
meson_setup_args = ["-Dblas=none"]
[[packages.sets.variants]]
cpython = "{verified_cpython}"
abi_tier = "cpython-abi"
target_triple = "wasm32-wasip1"
expected_identity_sha256 = "{"c" * 64}"
[[packages.sets.extensions]]
module = "numpy._core._multiarray_umath"
target = "_multiarray_umath"
python_exports = ["numpy"]
capabilities = ["module.extension.exec"]
provided_capsules = []
exclude_linked_static_libraries = []

[[packages]]
name = "scipy"
version = "{verified_scipy}"
[packages.source]
kind = "git"
commit = "{"b" * 40}"
[[packages.sets]]
name = "pact-witness"
seal_name = "scipy-witness"
build_dependency_group = "source-build-scipy"
use_pkg_config = true
required_config_tools = ["numpy-config", "pkg-config", "pybind11-config", "pythran-config"]
required_installed_files = ["scipy/__init__.py"]
meson_setup_args = ["-Dblas=none"]
[[packages.sets.variants]]
cpython = "{verified_cpython}"
abi_tier = "cpython-abi"
target_triple = "wasm32-wasip1"
expected_identity_sha256 = "{"d" * 64}"
[[packages.sets.extensions]]
module = "scipy.ndimage._nd_image"
target = "_nd_image"
python_exports = ["scipy"]
capabilities = ["module.extension.exec"]
provided_capsules = []
exclude_linked_static_libraries = []
''',
        encoding="utf-8",
    )
    scientific = root / "scientific.toml"
    scientific.write_text(
        f'''schema_version = {scientific_schema}
source_extension_registry = "{registry.name}"

[selection]
numpy = "{selected_numpy}"
scipy = "{selected_scipy}"
cpython = "{selected_cpython}"

[[verified]]
numpy = "{verified_numpy}"
scipy = "{verified_scipy}"
cpython = "{verified_cpython}"
extension_sets = ["numpy/pact-witness", "scipy/pact-witness"]
''',
        encoding="utf-8",
    )
    return scientific


def _load_tool(name: str, *, filename: str | None = None):
    spec = importlib.util.spec_from_file_location(
        name, TOOLS / f"{filename or name}.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_current_verified_stack_consumes_generic_registry() -> None:
    stack = resolve_scientific_stack()
    assert (stack.numpy, stack.scipy, stack.cpython) == ("2.5.1", "1.18.0", "3.12")
    assert [item.coordinate for item in stack.extension_sets] == [
        ("numpy", "2.5.1", "pact-witness"),
        ("scipy", "1.18.0", "pact-witness"),
    ]
    assert (
        stack.numpy_repo_ref
        == stack.source_extension_registry.package("numpy", "2.5.1").source.commit
    )
    verify_source_extension_abi_headers(
        scientific_witness_variant(stack=stack), repo_root=ROOT
    )


def test_scientific_witness_root_is_registered_variant_and_version_keyed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        set_registry,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path),
    )
    stack = resolve_scientific_stack()
    variant = scientific_witness_variant(stack=stack)
    assert scientific_witness_seal_root("numpy", variant=variant, stack=stack) == (
        tmp_path
        / "package-seals"
        / "numpy"
        / "2.5.1"
        / "variants"
        / "cpython-3.12"
        / "cpython-abi"
        / "wasm32-wasip1"
        / "pact_numpy_multiarray_sealed_for_witness"
    )


def test_scientific_custody_ignores_scratch_output_roots(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    custody = tmp_path / "custody"
    custody.mkdir()
    monkeypatch.setattr(
        set_registry,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=custody.resolve()),
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", r"D:\Molt")
    monkeypatch.setenv("MOLT_EXTERNAL_ARTIFACT_ROOTS", r"D:\Molt")
    assert scientific_custody_root() == custody.resolve()
    assert scientific_witness_seal_root(
        "numpy", variant=scientific_witness_variant()
    ).is_relative_to(custody.resolve())


def test_scientific_stack_exposes_registered_set_identity() -> None:
    stack = resolve_scientific_stack()
    extension_set = stack.extension_set("scipy", "pact-witness")
    variant = scientific_witness_variant(stack=stack)
    assert extension_set.package_version == "1.18.0"
    assert extension_set.source.commit == stack.scipy_repo_ref
    assert (
        source_extension_set_expected_identity(
            extension_set,
            variant=variant,
            registry=stack.source_extension_registry,
        )
        == extension_set.variants[0].expected_identity_sha256
    )


def test_numpy_seal_attestation_rejects_config_effective_drift(tmp_path: Path) -> None:
    seal = tmp_path / "seal"
    (seal / "numpy").mkdir(parents=True)
    (seal / "numpy/version.py").write_text(
        'version = "2.4.2"\n__version__ = version\n', encoding="utf-8"
    )
    with pytest.raises(ValueError, match=r"configured=2\.5\.1 effective=2\.4\.2"):
        attest_numpy_witness_seal(seal)


def test_unsupported_selection_fails_honestly_early(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _write_configs(tmp_path, selected_numpy="9.9.9")
    monkeypatch.setenv(CONFIG_ENV, str(config))
    with pytest.raises(
        ValueError,
        match=r"numpy 9\.9\.9/scipy 1\.18\.0/cpython 3\.12 .*verified-support matrix",
    ):
        resolve_scientific_stack()


def test_scientific_schema_v6_rejects_embedded_extension_authority(
    tmp_path: Path,
) -> None:
    config = _write_configs(tmp_path)
    config.write_text(
        config.read_text(encoding="utf-8")
        + '\n[[verified.extension_sets]]\npackage = "numpy"\n',
        encoding="utf-8",
    )
    with pytest.raises(
        ValueError, match="invalid scientific-stack config|keys are invalid"
    ):
        resolve_scientific_stack(config)


def test_scientific_schema_v5_is_rejected_without_compatibility_lane(
    tmp_path: Path,
) -> None:
    config = _write_configs(tmp_path, scientific_schema=5)
    with pytest.raises(ValueError, match="schema_version must be 6"):
        resolve_scientific_stack(config)


def test_config_only_version_change_preserves_pinned_queue_overlay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _write_configs(
        tmp_path,
        selected_numpy="9.9.9",
        selected_scipy="8.8.8",
        verified_numpy="9.9.9",
        verified_scipy="8.8.8",
    )
    monkeypatch.setenv(CONFIG_ENV, str(config))
    from tools.proof_queue_pkg import pact, state

    spec = pact._pact_witness_oracle_spec()
    command = list(spec["command"])
    assert command[command.index("--with-requirements") + 1] == (
        pact._PACT_WITNESS_REQUIREMENTS
    )
    requirements_text = (state.ROOT / pact._PACT_WITNESS_REQUIREMENTS).read_text(
        encoding="utf-8"
    )
    assert "numpy==2.5.1" in requirements_text
    assert "scipy==1.18.0" in requirements_text
    assert "numpy==9.9.9" not in requirements_text
    assert "scipy==8.8.8" not in requirements_text

    bench_manifest = _load_tool("bench_friends_manifest")
    _, suites = bench_manifest._load_manifest(
        ROOT / "bench" / "friends" / "manifest.toml"
    )
    numpy_suite = next(suite for suite in suites if suite.id == "numpy_off_the_shelf")
    assert numpy_suite.repo_ref == "a" * 40
    assert "numpy==9.9.9" in numpy_suite.runners["cpython"].run_cmd

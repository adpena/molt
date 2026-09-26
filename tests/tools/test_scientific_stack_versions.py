from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

import molt.cli.source_extension_set_registry as set_registry
import molt.scientific_stack_versions as scientific_stack_versions
from molt.cli.source_extension_set_registry import (
    source_extension_set_expected_identity,
    verify_source_extension_abi_headers,
)
from molt.scientific_stack_versions import (
    CONFIG_ENV,
    resolve_scientific_stack,
    scientific_custody_root,
    scientific_extension_seal_root,
    scientific_extension_variant,
    validate_scientific_extension_seals,
)

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"


def _variant_tables(cpython: str, targets: tuple[str, ...], *, digit: str) -> str:
    return "\n".join(
        f'''[[packages.sets.variants]]
cpython = "{cpython}"
abi_tier = "cpython-abi"
target_triple = "{target}"
expected_identity_sha256 = "{digit * 63}{index:x}"'''
        for index, target in enumerate(targets)
    )


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
    numpy_targets: tuple[str, ...] = ("wasm32-wasip1",),
    scipy_targets: tuple[str, ...] = ("wasm32-wasip1",),
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
{_variant_tables(verified_cpython, numpy_targets, digit="c")}
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
{_variant_tables(verified_cpython, scipy_targets, digit="d")}
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
        scientific_extension_variant("wasm", stack=stack), repo_root=ROOT
    )


def test_scientific_extension_root_is_registered_variant_and_version_keyed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        set_registry,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path),
    )
    stack = resolve_scientific_stack()
    variant = scientific_extension_variant("wasm", stack=stack)
    assert scientific_extension_seal_root("numpy", variant=variant, stack=stack) == (
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
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "scratch"))
    monkeypatch.setenv("MOLT_EXTERNAL_ARTIFACT_ROOTS", str(tmp_path / "scratch"))
    assert scientific_custody_root() == custody.resolve()
    assert scientific_extension_seal_root(
        "numpy", variant=scientific_extension_variant("wasm")
    ).is_relative_to(custody.resolve())


def test_scientific_stack_exposes_registered_set_identity() -> None:
    stack = resolve_scientific_stack()
    extension_set = stack.extension_set("scipy", "pact-witness")
    variant = scientific_extension_variant("wasm", stack=stack)
    assert extension_set.package_version == "1.18.0"
    assert extension_set.source.commit == stack.scipy_repo_ref
    assert source_extension_set_expected_identity(
        extension_set,
        variant=variant,
        registry=stack.source_extension_registry,
    ) == next(
        expectation.expected_identity_sha256
        for expectation in extension_set.variants
        if expectation.variant == variant
    )


def test_scientific_variants_use_target_resolver_and_both_registered_identities(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    targets = ("wasm32-wasip1", "x86_64-pc-windows-msvc")
    stack = resolve_scientific_stack(
        _write_configs(tmp_path, numpy_targets=targets, scipy_targets=targets)
    )
    monkeypatch.setattr(
        "molt.cli.source_extension_target._host_target_triple",
        lambda **_facts: "x86_64-pc-windows-msvc",
    )
    wasm = scientific_extension_variant("wasm", stack=stack)
    native = scientific_extension_variant("native", stack=stack)
    assert wasm.coordinate == ("3.12", "cpython-abi", "wasm32-wasip1")
    assert native.coordinate == (
        "3.12",
        "cpython-abi",
        "x86_64-pc-windows-msvc",
    )
    assert scientific_extension_variant("x86_64-pc-windows-msvc", stack=stack) == native
    for package, digit in (("numpy", "c"), ("scipy", "d")):
        extension_set = stack.extension_set(package, "pact-witness")
        assert (
            source_extension_set_expected_identity(
                extension_set, variant=wasm, registry=stack.source_extension_registry
            )
            == f"{digit * 63}0"
        )
        assert (
            source_extension_set_expected_identity(
                extension_set, variant=native, registry=stack.source_extension_registry
            )
            == f"{digit * 63}1"
        )


def test_scientific_stack_rejects_unpaired_target_variants(tmp_path: Path) -> None:
    config = _write_configs(
        tmp_path,
        numpy_targets=("wasm32-wasip1", "x86_64-pc-windows-msvc"),
        scipy_targets=("wasm32-wasip1",),
    )
    with pytest.raises(
        ValueError,
        match=r"variant coordinates differ: numpy-only=.*x86_64-pc-windows-msvc",
    ):
        resolve_scientific_stack(config)


def test_scientific_variant_rejects_unregistered_cross_target(tmp_path: Path) -> None:
    stack = resolve_scientific_stack(_write_configs(tmp_path))
    with pytest.raises(
        ValueError,
        match=r"no canonical identity.*numpy/2\.5\.1/pact-witness/3\.12/cpython-abi/x86_64-pc-windows-msvc",
    ):
        scientific_extension_variant("x86_64-pc-windows-msvc", stack=stack)
    with pytest.raises(ValueError, match="object-format policy"):
        scientific_extension_variant("x86_64-apple-ios", stack=stack)


def test_scientific_validation_requires_both_exact_registered_receipts(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    custody = tmp_path / "custody"
    monkeypatch.setattr(
        set_registry,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=custody),
    )
    stack = resolve_scientific_stack()
    variant = scientific_extension_variant("wasm", stack=stack)
    roots = {
        package: scientific_extension_seal_root(package, variant=variant, stack=stack)
        for package in ("numpy", "scipy")
    }
    for root in roots.values():
        root.mkdir(parents=True)
    calls: list[str] = []

    def validate(root: Path, extension_set, *, variant, registry):
        package = extension_set.package
        assert root == roots[package]
        assert variant == scientific_extension_variant("wasm", stack=stack)
        assert registry is stack.source_extension_registry
        assert extension_set.package_version == (
            "2.5.1" if package == "numpy" else "1.18.0"
        )
        calls.append(package)
        return SimpleNamespace(payload_root=root / "files")

    monkeypatch.setattr(
        scientific_stack_versions, "validate_source_extension_set_seal", validate
    )
    validated = validate_scientific_extension_seals("wasm", stack=stack)
    assert validated.variant == variant
    assert validated.payload_roots == (
        roots["numpy"] / "files",
        roots["scipy"] / "files",
    )
    assert validated.receipt("numpy") is validated.numpy
    assert validated.receipt("scipy") is validated.scipy
    assert calls == ["numpy", "scipy"]
    with pytest.raises(ValueError, match="no scientific extension seal"):
        validated.receipt("pandas")


def test_scientific_validation_rejects_missing_or_identity_invalid_pair(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    custody = tmp_path / "custody"
    monkeypatch.setattr(
        set_registry,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=custody),
    )
    stack = resolve_scientific_stack()
    variant = scientific_extension_variant("wasm", stack=stack)
    numpy_root = scientific_extension_seal_root("numpy", variant=variant, stack=stack)
    scipy_root = scientific_extension_seal_root("scipy", variant=variant, stack=stack)
    numpy_root.mkdir(parents=True)

    def admit_numpy(root: Path, extension_set, *, variant, registry):
        assert extension_set.package == "numpy"
        assert variant == scientific_extension_variant("wasm", stack=stack)
        assert registry is stack.source_extension_registry
        return SimpleNamespace(payload_root=root / "files")

    monkeypatch.setattr(
        scientific_stack_versions,
        "validate_source_extension_set_seal",
        admit_numpy,
    )
    with pytest.raises(ValueError, match="canonical scipy.*does not exist"):
        validate_scientific_extension_seals("wasm", stack=stack)

    scipy_root.mkdir(parents=True)
    calls: list[str] = []

    def reject_scipy(root: Path, extension_set, *, variant, registry):
        assert variant == scientific_extension_variant("wasm", stack=stack)
        assert registry is stack.source_extension_registry
        calls.append(extension_set.package)
        if extension_set.package == "scipy":
            raise ValueError("source-extension canonical identity mismatch")
        return SimpleNamespace(payload_root=root / "files")

    monkeypatch.setattr(
        scientific_stack_versions,
        "validate_source_extension_set_seal",
        reject_scipy,
    )
    with pytest.raises(
        ValueError, match="canonical scipy.*canonical identity mismatch"
    ):
        validate_scientific_extension_seals("wasm", stack=stack)
    assert calls == ["numpy", "scipy"]


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


def test_oracle_environment_group_pins_the_selected_stack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The locked `pact-witness` dependency group (the Pact witness lanes'
    oracle environment) pins exactly the selected stack versions, and a
    config-only version change cannot silently move it: the two authorities
    must agree, so drift is red here rather than a different oracle."""
    import tomllib

    from molt.scientific_stack_versions import PACT_WITNESS_DEPENDENCY_GROUP

    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    group = pyproject["dependency-groups"][PACT_WITNESS_DEPENDENCY_GROUP]
    selected = resolve_scientific_stack()
    assert sorted(group) == sorted(
        [selected.numpy_requirement, selected.scipy_requirement]
    )

    config = _write_configs(
        tmp_path,
        selected_numpy="9.9.9",
        selected_scipy="8.8.8",
        verified_numpy="9.9.9",
        verified_scipy="8.8.8",
    )
    monkeypatch.setenv(CONFIG_ENV, str(config))
    from tools.proof_queue_pkg import pact

    command = list(pact._pact_witness_oracle_spec()["command"])
    assert "--with-requirements" not in command
    assert command[-2:] == ["python", "tools/pact_witness_oracle.py"]
    assert "numpy==9.9.9" not in group
    assert "scipy==8.8.8" not in group

    bench_manifest = _load_tool("bench_friends_manifest")
    _, suites = bench_manifest._load_manifest(
        ROOT / "bench" / "friends" / "manifest.toml"
    )
    numpy_suite = next(suite for suite in suites if suite.id == "numpy_off_the_shelf")
    assert numpy_suite.repo_ref == "a" * 40
    assert "numpy==9.9.9" in numpy_suite.runners["cpython"].run_cmd

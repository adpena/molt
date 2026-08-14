from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

import molt.scientific_stack_versions as stack_versions
from molt.cli.source_extension_toolchain import _python_pc_text
from molt.scientific_stack_versions import (
    CONFIG_ENV,
    ScientificExtensionVariant,
    attest_numpy_witness_seal,
    resolve_scientific_stack,
    scientific_extension_set,
    scientific_extension_set_root,
    scientific_custody_root,
    scientific_witness_seal_root,
    scientific_witness_variant,
    verify_cpython_abi_headers,
    verify_source_checkout,
)

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"


def _write_config(
    path: Path,
    *,
    selected_numpy: str,
    selected_scipy: str = "1.18.0",
    selected_cpython: str = "3.12",
    verified_numpy: str = "2.5.1",
    verified_scipy: str = "1.18.0",
    verified_cpython: str = "3.12",
    schema_version: int = 4,
) -> None:
    path.write_text(
        f'''schema_version = {schema_version}

[selection]
numpy = "{selected_numpy}"
scipy = "{selected_scipy}"
cpython = "{selected_cpython}"

[[verified]]
numpy = "{verified_numpy}"
scipy = "{verified_scipy}"
cpython = "{verified_cpython}"
numpy_repo_ref = "numpy-ref"
scipy_repo_ref = "scipy-ref"
[[verified.extension_sets]]
package = "numpy"
name = "pact-witness"
seal_name = "pact_numpy_multiarray_sealed_for_witness"
expected_identity_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
build_dependency_group = "source-build-numpy"
use_pkg_config = false
required_installed_files = ["numpy/__config__.py", "numpy/__init__.py", "numpy/version.py"]
meson_setup_args = ["-Dblas=none", "-Dlapack=none"]

[[verified.extension_sets.extensions]]
module = "numpy._core._multiarray_umath"
target = "_multiarray_umath"
python_exports = ["numpy"]
capabilities = ["module.extension.exec"]
provided_capsules = ["numpy.core._multiarray_umath._ARRAY_API"]
exclude_linked_static_libraries = []

[[verified.extension_sets]]
package = "scipy"
name = "pact-witness"
seal_name = "pact_scipy_witness"
expected_identity_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
build_dependency_group = "source-build-scipy"
use_pkg_config = true
required_installed_files = ["scipy/__config__.py", "scipy/__init__.py", "scipy/version.py"]
meson_setup_args = ["-Dblas=none", "-Dlapack=none"]

[[verified.extension_sets.extensions]]
module = "scipy.ndimage._nd_image"
target = "_nd_image"
python_exports = ["scipy"]
capabilities = ["module.extension.exec"]
provided_capsules = []
exclude_linked_static_libraries = []
''',
        encoding="utf-8",
    )


def _load_tool(name: str, *, filename: str | None = None):
    spec = importlib.util.spec_from_file_location(
        name, TOOLS / f"{filename or name}.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_current_verified_stack_and_cpython_abi_are_aligned() -> None:
    stack = resolve_scientific_stack()
    assert (stack.numpy, stack.scipy, stack.cpython) == ("2.5.1", "1.18.0", "3.12")
    verify_cpython_abi_headers(stack=stack, repo_root=ROOT)


def test_numpy_witness_seal_root_is_variant_and_version_keyed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        stack_versions,
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
        stack_versions,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=custody.resolve()),
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", r"D:\Molt")
    monkeypatch.setenv("MOLT_EXTERNAL_ARTIFACT_ROOTS", r"D:\Molt")

    assert scientific_custody_root() == custody.resolve()
    assert scientific_witness_seal_root(
        "numpy", variant=scientific_witness_variant()
    ).is_relative_to(custody.resolve())


def test_scipy_extension_set_and_seal_root_are_typed_and_version_keyed(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        stack_versions,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path),
    )
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set("scipy", "pact-witness", stack=stack)
    variant = scientific_witness_variant(stack=stack)

    assert (extension_set.package, extension_set.name, extension_set.seal_name) == (
        "scipy",
        "pact-witness",
        "pact_scipy_witness",
    )
    assert extension_set.build_dependency_group == "source-build-scipy"
    assert extension_set.meson_setup_args == (
        "-Dblas=none",
        "-Dlapack=none",
        "-D_without-fortran=true",
        "-Duse-pythran=true",
    )
    assert [
        (
            extension.module,
            extension.target,
            extension.python_exports,
            extension.capabilities,
        )
        for extension in extension_set.extensions
    ] == [
        (
            "scipy.ndimage._nd_image",
            "_nd_image",
            ("scipy",),
            ("module.extension.exec",),
        ),
        (
            "scipy.ndimage._ni_label",
            "_ni_label",
            ("scipy.ndimage._ni_label",),
            ("module.extension.exec",),
        ),
        (
            "scipy.ndimage._rank_filter_1d",
            "_rank_filter_1d",
            ("scipy.ndimage._rank_filter_1d",),
            ("module.extension.exec",),
        ),
        (
            "scipy._lib._ccallback_c",
            "_ccallback_c",
            ("scipy._lib._ccallback_c",),
            ("module.extension.exec",),
        ),
    ]
    expected = (
        tmp_path
        / "package-seals"
        / "scipy"
        / "1.18.0"
        / "variants"
        / "cpython-3.12"
        / "cpython-abi"
        / "wasm32-wasip1"
        / "pact_scipy_witness"
    )
    assert (
        scientific_extension_set_root(extension_set, variant=variant, stack=stack)
        == expected
    )
    assert (
        scientific_witness_seal_root("scipy", variant=variant, stack=stack) == expected
    )


def test_scientific_native_and_wasm_extension_variants_coexist(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        stack_versions,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path),
    )
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set("numpy", "pact-witness", stack=stack)
    wasm = scientific_witness_variant(stack=stack)
    native = ScientificExtensionVariant(
        cpython=stack.cpython,
        abi_tier="cpython-abi",
        target_triple="x86_64-unknown-linux-gnu",
    )

    wasm_root = scientific_extension_set_root(extension_set, variant=wasm, stack=stack)
    native_root = scientific_extension_set_root(
        extension_set, variant=native, stack=stack
    )
    for root, marker in ((wasm_root, "wasm"), (native_root, "native")):
        root.mkdir(parents=True)
        (root / "variant.txt").write_text(marker, encoding="utf-8")

    assert wasm_root != native_root
    assert (wasm_root / "variant.txt").read_text(encoding="utf-8") == "wasm"
    assert (native_root / "variant.txt").read_text(encoding="utf-8") == "native"
    assert wasm_root.parts[-5:-1] == (
        "variants",
        "cpython-3.12",
        "cpython-abi",
        "wasm32-wasip1",
    )
    assert native_root.parts[-5:-1] == (
        "variants",
        "cpython-3.12",
        "cpython-abi",
        "x86_64-unknown-linux-gnu",
    )


def test_scientific_extension_root_rejects_cpython_variant_mismatch() -> None:
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set("numpy", "pact-witness", stack=stack)
    mismatched = ScientificExtensionVariant(
        cpython="3.13",
        abi_tier="cpython-abi",
        target_triple="wasm32-wasip1",
    )

    with pytest.raises(
        ValueError,
        match=r"variant CPython 3\.13 .* verified-stack CPython 3\.12",
    ):
        scientific_extension_set_root(
            extension_set,
            variant=mismatched,
            stack=stack,
        )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("cpython", " 3.12"),
        ("abi_tier", "CPython-ABI"),
        ("target_triple", "x86_64 unknown linux gnu"),
    ],
)
def test_scientific_extension_variant_rejects_noncanonical_components(
    field: str,
    value: str,
) -> None:
    values = {
        "cpython": "3.12",
        "abi_tier": "cpython-abi",
        "target_triple": "wasm32-wasip1",
    }
    values[field] = value

    with pytest.raises(ValueError, match=rf"variant {field} must be a canonical"):
        ScientificExtensionVariant(**values)


def test_scientific_extension_roots_require_explicit_variant_custody() -> None:
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set("numpy", "pact-witness", stack=stack)

    with pytest.raises(TypeError, match="variant"):
        scientific_extension_set_root(extension_set, stack=stack)  # type: ignore[call-arg]
    with pytest.raises(TypeError, match="variant"):
        scientific_witness_seal_root("numpy", stack=stack)  # type: ignore[call-arg]
    assert not hasattr(stack_versions, "numpy_witness_seal_root")
    assert not hasattr(stack_versions, "scipy_witness_seal_root")


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
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="9.9.9")
    monkeypatch.setenv(CONFIG_ENV, str(config))

    with pytest.raises(
        ValueError,
        match=r"numpy 9\.9\.9/scipy 1\.18\.0/cpython 3\.12 .*verified-support matrix",
    ):
        resolve_scientific_stack()

    from tools.proof_queue_pkg import pact, state

    monkeypatch.setattr(pact, "_pact_witness_env_overrides", lambda _root: {})
    spec = pact._pact_witness_acceptance_spec()
    command = list(spec["command"])
    requirements = state.ROOT / pact._PACT_WITNESS_REQUIREMENTS
    canonical_stack = resolve_scientific_stack(
        state.ROOT / "config/scientific_stack_versions.toml"
    )
    assert command[command.index("--with-requirements") + 1] == (
        pact._PACT_WITNESS_REQUIREMENTS
    )
    requirements_text = requirements.read_text(encoding="utf-8")
    assert canonical_stack.numpy_requirement in requirements_text
    assert canonical_stack.scipy_requirement in requirements_text
    assert pact._PACT_WITNESS_REQUIREMENTS in spec["scopes"]
    assert spec["env_overrides"][CONFIG_ENV] == str(
        state.ROOT / "config/scientific_stack_versions.toml"
    )


def test_schema_v4_rejects_legacy_scipy_root_fields(tmp_path: Path) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1")
    text = config.read_text(encoding="utf-8")
    config.write_text(
        text.replace(
            'scipy_repo_ref = "scipy-ref"\n',
            'scipy_repo_ref = "scipy-ref"\n'
            'scipy_additional_seal_roots = ["tmp/legacy"]\n',
        ),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="removed schema-v1 fields"):
        resolve_scientific_stack(config)


@pytest.mark.parametrize(
    ("replacement", "problem"),
    [
        ("", "capabilities must be a string array"),
        (
            "capabilities = []\n",
            "must include 'module.extension.exec'",
        ),
        (
            'capabilities = ["fs.read"]\n',
            "must include 'module.extension.exec'",
        ),
        (
            'capabilities = ["net", "fs.read"]\n',
            "must be sorted and contain no duplicate capabilities",
        ),
        (
            'capabilities = ["fs.read", "fs.read"]\n',
            "must be sorted and contain no duplicate capabilities",
        ),
    ],
)
def test_schema_v4_requires_canonical_module_capability_authority(
    tmp_path: Path, replacement: str, problem: str
) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1")
    config.write_text(
        config.read_text(encoding="utf-8").replace(
            'capabilities = ["module.extension.exec"]\n', replacement, 1
        ),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match=problem):
        resolve_scientific_stack(config)


def test_schema_v3_is_rejected_without_a_compatibility_lane(tmp_path: Path) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1", schema_version=3)

    with pytest.raises(ValueError, match="schema_version must be 4"):
        resolve_scientific_stack(config)


@pytest.mark.parametrize("dirty_kind", ["tracked", "untracked"])
def test_source_checkout_attestation_rejects_every_dirty_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    dirty_kind: str,
) -> None:
    source = tmp_path / "scipy"
    source.mkdir()
    run_guarded_test_process(["git", "init", "-q", str(source)], check=True)
    run_guarded_test_process(
        ["git", "-C", str(source), "config", "user.email", "molt@example.invalid"],
        check=True,
    )
    run_guarded_test_process(
        ["git", "-C", str(source), "config", "user.name", "Molt Test"],
        check=True,
    )
    tracked = source / "scipy.c"
    tracked.write_text("int scipy(void) { return 1; }\n", encoding="utf-8")
    run_guarded_test_process(["git", "-C", str(source), "add", "scipy.c"], check=True)
    run_guarded_test_process(
        ["git", "-C", str(source), "commit", "-q", "-m", "source"], check=True
    )
    head = run_guarded_test_process(
        ["git", "-C", str(source), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1")
    config.write_text(
        config.read_text(encoding="utf-8").replace(
            'scipy_repo_ref = "scipy-ref"', f'scipy_repo_ref = "{head}"'
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv(CONFIG_ENV, str(config))

    verify_source_checkout("scipy", source)
    if dirty_kind == "tracked":
        tracked.write_text("int scipy(void) { return 2; }\n", encoding="utf-8")
    else:
        (source / "untracked.c").write_text("int drift;\n", encoding="utf-8")

    with pytest.raises(ValueError, match="not a clean immutable input"):
        verify_source_checkout("scipy", source)


def test_config_only_version_change_preserves_pinned_queue_overlay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(
        config,
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
    assert pact._PACT_WITNESS_REQUIREMENTS in spec["scopes"]

    bench_manifest = _load_tool("bench_friends_manifest")
    _, suites = bench_manifest._load_manifest(
        ROOT / "bench" / "friends" / "manifest.toml"
    )
    numpy_suite = next(suite for suite in suites if suite.id == "numpy_off_the_shelf")
    assert numpy_suite.repo_ref == "numpy-ref"
    assert "numpy==9.9.9" in numpy_suite.runners["cpython"].run_cmd
    assert "9.9.9" in numpy_suite.runners["cpython"].run_cmd

    python_pc = _python_pc_text(molt_root=ROOT, abi_tier="cpython-abi")
    assert "Version: 3.12" in python_pc

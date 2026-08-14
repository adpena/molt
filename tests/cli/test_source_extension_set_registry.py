from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

import molt.cli.source_extension_set_registry as registry_module
from molt.cli.source_extension_set_registry import (
    CONFIG_ENV,
    SourceExtensionVariant,
    load_source_extension_registry,
    source_extension_set_expected_identity,
    source_extension_set_root,
    verify_source_extension_checkout,
)
from molt.target_python import TargetPythonVersion
from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[2]


def _registry_text(*, commit: str = "a" * 40) -> str:
    return f'''schema_version = 1

[[packages]]
name = "demo"
version = "1.2.3"
[packages.source]
kind = "git"
commit = "{commit}"
[[packages.sets]]
name = "core"
seal_name = "demo-core"
build_dependency_group = "source-build-demo"
use_pkg_config = false
required_config_tools = []
required_installed_files = ["demo/__init__.py"]
meson_setup_args = ["--buildtype=release"]
[[packages.sets.variants]]
cpython = "3.12"
abi_tier = "cpython-abi"
target_triple = "wasm32-wasip1"
expected_identity_sha256 = "{"b" * 64}"
[[packages.sets.extensions]]
module = "demo._core"
target = "_core"
python_exports = ["demo"]
capabilities = ["module.extension.exec"]
provided_capsules = []
exclude_linked_static_libraries = []
'''


def _write_registry(path: Path, *, commit: str = "a" * 40) -> Path:
    path.write_text(_registry_text(commit=commit), encoding="utf-8")
    return path


def test_canonical_registry_has_exact_versioned_package_sets() -> None:
    registry = load_source_extension_registry()
    assert registry.schema_version == 1
    assert [(item.name, item.version) for item in registry.packages] == [
        ("numpy", "2.5.1"),
        ("scipy", "1.18.0"),
    ]
    numpy = registry.extension_set("numpy", "2.5.1", "pact-witness")
    assert numpy.source.commit == "5e1d03ffac5f2c0a9c39bfcaa9fc853b2b83151e"
    assert numpy.variants[0].variant.target_python == TargetPythonVersion(3, 12, 0)


def test_registry_environment_override_is_the_only_alternate_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _write_registry(tmp_path / "registry.toml")
    monkeypatch.setenv(CONFIG_ENV, str(config))
    assert load_source_extension_registry().path == config.resolve()


def test_root_requires_registered_full_variant_coordinate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _write_registry(tmp_path / "registry.toml")
    registry = load_source_extension_registry(config)
    extension_set = registry.extension_set("demo", "1.2.3", "core")
    monkeypatch.setattr(
        registry_module,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path / "custody"),
    )
    wasm = extension_set.variants[0].variant
    assert source_extension_set_root(
        extension_set, variant=wasm, registry=registry
    ) == (
        tmp_path
        / "custody/package-seals/demo/1.2.3/variants/cpython-3.12"
        / "cpython-abi/wasm32-wasip1/demo-core"
    )
    native = SourceExtensionVariant(
        target_python=TargetPythonVersion(3, 12, 0),
        abi_tier="cpython-abi",
        target_triple="x86_64-unknown-linux-gnu",
    )
    with pytest.raises(ValueError, match="no canonical identity is registered"):
        source_extension_set_root(extension_set, variant=native, registry=registry)


def test_variant_requires_target_python_authority() -> None:
    with pytest.raises(TypeError, match="TargetPythonVersion"):
        SourceExtensionVariant(  # type: ignore[arg-type]
            target_python="3.12",
            abi_tier="cpython-abi",
            target_triple="wasm32-wasip1",
        )


@pytest.mark.parametrize(
    ("needle", "replacement", "problem"),
    [
        ("schema_version = 1", "schema_version = 2", "schema_version must be 1"),
        (
            'commit = "' + "a" * 40 + '"',
            'commit = "NOT-A-COMMIT"',
            "lowercase 40-hex commit",
        ),
        (
            'expected_identity_sha256 = "' + "b" * 64 + '"',
            'expected_identity_sha256 = "' + "B" * 64 + '"',
            "lowercase SHA-256",
        ),
        (
            'capabilities = ["module.extension.exec"]',
            'capabilities = ["fs.read"]',
            "must include 'module.extension.exec'",
        ),
        (
            'required_installed_files = ["demo/__init__.py"]',
            'required_installed_files = ["../escape.py"]',
            "root-relative POSIX path",
        ),
        (
            'target = "_core"',
            'target = "CON"',
            "safe filename",
        ),
        (
            'module = "demo._core"',
            'module = "demo.class"',
            "not import syntax",
        ),
    ],
)
def test_registry_rejects_noncanonical_nested_authority(
    tmp_path: Path, needle: str, replacement: str, problem: str
) -> None:
    config = tmp_path / "registry.toml"
    config.write_text(_registry_text().replace(needle, replacement), encoding="utf-8")
    with pytest.raises(ValueError, match=problem):
        load_source_extension_registry(config)


def test_registry_rejects_case_folded_artifact_collision(tmp_path: Path) -> None:
    config = _write_registry(tmp_path / "registry.toml")
    config.write_text(
        config.read_text(encoding="utf-8")
        + """
[[packages.sets.extensions]]
module = "demo._other"
target = "_CORE"
python_exports = ["demo"]
capabilities = ["module.extension.exec"]
provided_capsules = []
exclude_linked_static_libraries = []
""",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="artifact collision"):
        load_source_extension_registry(config)


def test_expected_identity_rejects_forged_set(tmp_path: Path) -> None:
    config = _write_registry(tmp_path / "registry.toml")
    registry = load_source_extension_registry(config)
    extension_set = registry.extension_set("demo", "1.2.3", "core")
    forged = type(extension_set)(
        **{
            **{
                field: getattr(extension_set, field)
                for field in extension_set.__dataclass_fields__
            },
            "seal_name": "forged",
        }
    )
    with pytest.raises(ValueError, match="differs from the registry authority"):
        source_extension_set_expected_identity(
            forged,
            variant=extension_set.variants[0].variant,
            registry=registry,
        )


@pytest.mark.parametrize("dirty_kind", ["tracked", "untracked"])
def test_source_checkout_attestation_rejects_every_dirty_input(
    tmp_path: Path, dirty_kind: str
) -> None:
    source = tmp_path / "source"
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
    tracked = source / "extension.c"
    tracked.write_text("int demo(void) { return 1; }\n", encoding="utf-8")
    run_guarded_test_process(
        ["git", "-C", str(source), "add", "extension.c"], check=True
    )
    run_guarded_test_process(
        ["git", "-C", str(source), "commit", "-q", "-m", "source"], check=True
    )
    head = run_guarded_test_process(
        ["git", "-C", str(source), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    registry = load_source_extension_registry(
        _write_registry(tmp_path / "registry.toml", commit=head)
    )
    extension_set = registry.extension_set("demo", "1.2.3", "core")
    verify_source_extension_checkout(extension_set, source, registry=registry)
    if dirty_kind == "tracked":
        tracked.write_text("int demo(void) { return 2; }\n", encoding="utf-8")
    else:
        (source / "untracked.c").write_text("int drift;\n", encoding="utf-8")
    with pytest.raises(ValueError, match="not a clean immutable input"):
        verify_source_extension_checkout(extension_set, source, registry=registry)

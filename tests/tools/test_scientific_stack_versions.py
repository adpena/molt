from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

from molt.cli.source_extension_toolchain import _python_pc_text
from molt.scientific_stack_versions import (
    CONFIG_ENV,
    attest_numpy_witness_seal,
    numpy_witness_seal_root,
    resolve_scientific_stack,
    scientific_extension_set,
    scientific_extension_set_root,
    scipy_witness_seal_root,
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
) -> None:
    path.write_text(
        f'''schema_version = 3

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
numpy_seal_root_candidates = ["tmp/numpy-seal"]

[[verified.extension_sets]]
package = "scipy"
name = "pact-witness"
seal_name = "pact_scipy_witness"
meson_setup_args = ["-Dblas=none", "-Dlapack=none"]

[[verified.extension_sets.extensions]]
module = "scipy.ndimage._nd_image"
target = "_nd_image"
python_exports = ["scipy"]
capabilities = []
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


def test_numpy_seal_root_is_version_keyed(tmp_path: Path) -> None:
    stack = resolve_scientific_stack()
    assert numpy_witness_seal_root(stack=stack, artifact_root=tmp_path) == (
        tmp_path
        / "package-seals"
        / "numpy"
        / "2.5.1"
        / "pact_numpy_multiarray_sealed_for_witness"
    )


def test_scipy_extension_set_and_seal_root_are_typed_and_version_keyed(
    tmp_path: Path,
) -> None:
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set("scipy", "pact-witness", stack=stack)

    assert (extension_set.package, extension_set.name, extension_set.seal_name) == (
        "scipy",
        "pact-witness",
        "pact_scipy_witness",
    )
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
        ("scipy.ndimage._nd_image", "_nd_image", ("scipy",), ()),
        (
            "scipy.ndimage._ni_label",
            "_ni_label",
            ("scipy.ndimage._ni_label",),
            (),
        ),
        (
            "scipy.ndimage._rank_filter_1d",
            "_rank_filter_1d",
            ("scipy.ndimage._rank_filter_1d",),
            (),
        ),
        (
            "scipy._lib._ccallback_c",
            "_ccallback_c",
            ("scipy._lib._ccallback_c",),
            (),
        ),
    ]
    expected = tmp_path / "package-seals" / "scipy" / "1.18.0" / "pact_scipy_witness"
    assert (
        scientific_extension_set_root(
            extension_set, stack=stack, artifact_root=tmp_path
        )
        == expected
    )
    assert scipy_witness_seal_root(stack=stack, artifact_root=tmp_path) == expected


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

    proof_queue = _load_tool("proof_queue")
    monkeypatch.setattr(proof_queue, "_pact_witness_env_overrides", lambda _root: {})
    spec = proof_queue._pact_witness_acceptance_spec()
    assert "numpy==2.5.1" in spec["command"]
    assert spec["env_overrides"][CONFIG_ENV] == str(
        proof_queue.ROOT / "config/scientific_stack_versions.toml"
    )


def test_schema_v3_rejects_legacy_scipy_root_fields(tmp_path: Path) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1")
    text = config.read_text(encoding="utf-8")
    config.write_text(
        text.replace(
            'numpy_seal_root_candidates = ["tmp/numpy-seal"]\n',
            'numpy_seal_root_candidates = ["tmp/numpy-seal"]\n'
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
            'capabilities = ["net", "fs.read"]\n',
            "must be sorted and contain no duplicate capabilities",
        ),
        (
            'capabilities = ["fs.read", "fs.read"]\n',
            "must be sorted and contain no duplicate capabilities",
        ),
    ],
)
def test_schema_v3_requires_canonical_module_capability_authority(
    tmp_path: Path, replacement: str, problem: str
) -> None:
    config = tmp_path / "scientific.toml"
    _write_config(config, selected_numpy="2.5.1")
    config.write_text(
        config.read_text(encoding="utf-8").replace(
            "capabilities = []\n", replacement, 1
        ),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match=problem):
        resolve_scientific_stack(config)


@pytest.mark.parametrize("dirty_kind", ["tracked", "untracked"])
def test_source_checkout_attestation_rejects_every_dirty_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    dirty_kind: str,
) -> None:
    source = tmp_path / "scipy"
    source.mkdir()
    subprocess.run(["git", "init", "-q", str(source)], check=True)
    subprocess.run(
        ["git", "-C", str(source), "config", "user.email", "molt@example.invalid"],
        check=True,
    )
    subprocess.run(
        ["git", "-C", str(source), "config", "user.name", "Molt Test"],
        check=True,
    )
    tracked = source / "scipy.c"
    tracked.write_text("int scipy(void) { return 1; }\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(source), "add", "scipy.c"], check=True)
    subprocess.run(
        ["git", "-C", str(source), "commit", "-q", "-m", "source"], check=True
    )
    head = subprocess.run(
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


def test_config_only_version_change_propagates_to_consumers(
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

    proof_queue = _load_tool("proof_queue_config_propagation", filename="proof_queue")
    command = proof_queue._pact_witness_oracle_spec()["command"]
    assert "numpy==9.9.9" in command
    assert "scipy==8.8.8" in command

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

from __future__ import annotations

import textwrap
from pathlib import Path

import pytest

import tools.pact_witness_scipy_generated_modules as generated
import tools.pact_witness_scipy_python_closure as closure


_FAKE_GITVERSION = textwrap.dedent(
    '''
    import argparse
    import os
    import textwrap


    def init_version():
        here = os.path.dirname(__file__)
        pyproject = os.path.join(here, "..", "pyproject.toml")
        with open(pyproject) as fid:
            for line in fid:
                if line.startswith("version ="):
                    return line.split("=", 1)[1].strip().strip("\\"'")
        raise SystemExit("no version in pyproject.toml")


    if __name__ == "__main__":
        parser = argparse.ArgumentParser()
        parser.add_argument("--write")
        parser.add_argument("--meson-dist", action="store_true")
        args = parser.parse_args()
        version = init_version()
        with open(args.write, "w") as fid:
            fid.write(textwrap.dedent(f\'\'\'
                version = "{version}"
                __version__ = version
                full_version = version
                git_revision = "deadbeef"
                release = 'dev' not in version and '+' not in version
                short_version = version.split("+")[0]
            \'\'\'))
    '''
)

# A fully substituted (no @VARS@) meson __config__.py build output.
_FAKE_CONFIG = 'def show(mode="stdout"):\n    return None\n'


def _make_repo(tmp_path: Path, *, with_config: bool = True) -> Path:
    repo = tmp_path / "repo"
    scipy_src = repo / closure._SCIPY_SOURCE_REL / "scipy"
    # A SciPy-shaped source tree with nested pure-Python submodules, a tests/ dir
    # that must be excluded, and the _external/packaging_version src/ relocation.
    scipy_src.mkdir(parents=True)
    (scipy_src / "__init__.py").write_text(
        "from scipy.version import version\n"
        "from scipy._external.packaging_version.version import parse\n"
        "from . import _distributor_init\n",
        encoding="utf-8",
    )
    (scipy_src / "_distributor_init.py").write_text("PASS = 1\n", encoding="utf-8")
    (scipy_src / "ndimage").mkdir()
    (scipy_src / "ndimage" / "__init__.py").write_text(
        "from ._filters import gaussian\n", encoding="utf-8"
    )
    (scipy_src / "ndimage" / "_filters.py").write_text(
        "def gaussian():\n    return 1\n", encoding="utf-8"
    )
    (scipy_src / "ndimage" / "tests").mkdir()
    (scipy_src / "ndimage" / "tests" / "__init__.py").write_text("", encoding="utf-8")
    (scipy_src / "ndimage" / "tests" / "test_filters.py").write_text(
        "SHOULD_NOT_BE_STAGED = True\n", encoding="utf-8"
    )
    # _external/packaging_version: sources live under src/ but install one level up.
    pv = scipy_src / "_external" / "packaging_version"
    (pv / "src").mkdir(parents=True)
    (scipy_src / "_external" / "__init__.py").write_text("", encoding="utf-8")
    (pv / "src" / "version.py").write_text(
        "class Version:\n    pass\n\n\ndef parse(v):\n    return Version()\n",
        encoding="utf-8",
    )
    (pv / "src" / "_structures.py").write_text("INFINITY = object()\n", encoding="utf-8")
    (pv / "meson.build").write_text(
        "python_sources = files(\n"
        "  'src/_structures.py',\n"
        "  'src/version.py',\n"
        ")\n\n"
        "py3.install_sources(python_sources, subdir: 'scipy/_external/packaging_version')\n",
        encoding="utf-8",
    )
    # SciPy's own version generator lives at the checkout root (tools/), + pyproject.
    (repo / closure._SCIPY_SOURCE_REL / "pyproject.toml").write_text(
        '[project]\nversion = "9.9.9"\n', encoding="utf-8"
    )
    gitversion = repo / closure._SCIPY_SOURCE_REL / "tools" / "gitversion.py"
    gitversion.parent.mkdir(parents=True)
    gitversion.write_text(_FAKE_GITVERSION, encoding="utf-8")
    # A sealed root that owns only the C-ext-adjacent files + the native artifact.
    seal_scipy = repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy"
    (seal_scipy / "ndimage").mkdir(parents=True)
    (seal_scipy / "__init__.py").write_text("PARTIAL = 1\n", encoding="utf-8")
    (seal_scipy / "ndimage" / "__init__.py").write_text("", encoding="utf-8")
    (seal_scipy / "ndimage" / "_nd_image.molt.wasm").write_bytes(b"\0asm")
    if with_config:
        build_cfg = (
            repo
            / "tmp/pact_scipy_ndimage_meson_wasm_build_generated/scipy/__config__.py"
        )
        build_cfg.parent.mkdir(parents=True)
        build_cfg.write_text(_FAKE_CONFIG, encoding="utf-8")
    return repo


def test_stage_mirrors_full_pure_python_subtree_into_seal(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    seal = repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy"

    assert not (seal / "_distributor_init.py").exists()
    assert not (seal / "ndimage" / "_filters.py").exists()

    closure.stage(repo)

    assert (seal / "_distributor_init.py").is_file()
    assert (seal / "ndimage" / "_filters.py").is_file()
    assert (seal / "_external" / "__init__.py").is_file()
    # Generated modules materialized on top.
    assert 'version = "9.9.9"' in (seal / "version.py").read_text(encoding="utf-8")
    assert (seal / "__config__.py").is_file()


def test_stage_applies_meson_install_rename_for_packaging_version(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    seal = repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy"
    closure.stage(repo)
    pv = seal / "_external" / "packaging_version"
    # Installed layout: src/ dropped (scipy/__init__ imports packaging_version.version).
    assert (pv / "version.py").is_file()
    assert (pv / "_structures.py").is_file()
    assert not (pv / "src" / "version.py").exists()
    # The relocated version.py is real scipy source, NOT the package-root generator.
    assert "class Version" in (pv / "version.py").read_text(encoding="utf-8")


def test_stage_excludes_tests_and_preserves_c_extension(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    seal = repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy"
    closure.stage(repo)
    assert not (seal / "ndimage" / "tests" / "test_filters.py").exists()
    assert (seal / "ndimage" / "_nd_image.molt.wasm").read_bytes() == b"\0asm"


def test_check_flags_partial_package_then_passes_after_stage(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    problems = closure.check(repo)
    assert problems and any("missing staged module" in p for p in problems)

    closure.stage(repo)
    assert closure.check(repo) == []


def test_check_detects_source_drift(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    closure.stage(repo)
    seal_mod = (
        repo
        / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy/_distributor_init.py"
    )
    seal_mod.write_text("PASS = 2  # drifted\n", encoding="utf-8")
    problems = closure.check(repo)
    assert any("stale staged module" in p for p in problems)


def test_missing_scipy_source_fails_closed(tmp_path: Path) -> None:
    repo = tmp_path / "empty"
    repo.mkdir()
    with pytest.raises(closure.ClosureStagingError):
        closure.stage(repo)


def test_package_root_version_not_clobbered_by_source_mirror(tmp_path: Path) -> None:
    # A stray scipy/version.py in the source tree must not overwrite the generator
    # authority (the package-root version.py is a protected relpath).
    repo = _make_repo(tmp_path)
    scipy_src = repo / closure._SCIPY_SOURCE_REL / "scipy"
    (scipy_src / "version.py").write_text("version = 'BOGUS'\n", encoding="utf-8")
    closure.stage(repo)
    seal_version = (
        repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy/version.py"
    )
    text = seal_version.read_text(encoding="utf-8")
    assert "BOGUS" not in text
    assert 'version = "9.9.9"' in text
    _ = generated  # generator authority exercised via closure.stage


def test_config_blocked_fails_closed_but_stages_pure_python_and_version(
    tmp_path: Path,
) -> None:
    # No authoritative meson __config__.py output: stage must persist the
    # pure-Python subtree + version.py, then fail closed naming __config__.py.
    repo = _make_repo(tmp_path, with_config=False)
    seal = repo / "tmp/pact_scipy_ndimage_sealed_for_witness_next/scipy"
    # stage() raises the GeneratedModuleError the closure module actually imports.
    with pytest.raises(closure._generated.GeneratedModuleError) as excinfo:
        closure.stage(repo)
    assert "__config__.py" in str(excinfo.value)
    assert "meson" in str(excinfo.value).lower()
    # Real increment persisted despite the config blocker.
    assert (seal / "_distributor_init.py").is_file()
    assert 'version = "9.9.9"' in (seal / "version.py").read_text(encoding="utf-8")
    assert not (seal / "__config__.py").exists()
    # check reports the __config__ blocker precisely.
    problems = closure.check(repo)
    assert any("__config__.py" in p for p in problems)

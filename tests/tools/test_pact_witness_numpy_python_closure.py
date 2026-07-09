from __future__ import annotations

import textwrap
from pathlib import Path

import pytest

import tools.pact_witness_numpy_generated_modules as generated
import tools.pact_witness_numpy_python_closure as closure


_FAKE_GITVERSION = textwrap.dedent(
    '''
    import argparse
    import os
    import textwrap


    def init_version():
        here = os.path.dirname(__file__)
        pyproject = os.path.join(here, "..", "..", "pyproject.toml")
        with open(pyproject) as fid:
            for line in fid:
                if line.startswith("version ="):
                    return line.split("=", 1)[1].strip().strip("\\"'")
        raise SystemExit("no version in pyproject.toml")


    if __name__ == "__main__":
        parser = argparse.ArgumentParser()
        parser.add_argument("--write")
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

_FAKE_CONFIG = 'def show_config(mode="stdout"):\n    return None\n'


def _make_repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    numpy_src = repo / closure._NUMPY_SOURCE_REL / "numpy"
    # A NumPy-shaped source tree with nested pure-Python submodules and a
    # tests/ dir that must be excluded from the mirror.
    (numpy_src / "_build_utils").mkdir(parents=True)
    (numpy_src / "__init__.py").write_text(
        "from . import version\nfrom ._globals import _NoValue\nfrom . import lib\n",
        encoding="utf-8",
    )
    (numpy_src / "_globals.py").write_text("_NoValue = object()\n", encoding="utf-8")
    (numpy_src / "_expired_attrs_2_0.py").write_text(
        "__expired_attributes__ = {}\n", encoding="utf-8"
    )
    (numpy_src / "lib").mkdir()
    (numpy_src / "lib" / "__init__.py").write_text("from . import _impl\n", encoding="utf-8")
    (numpy_src / "lib" / "_impl.py").write_text("X = 1\n", encoding="utf-8")
    (numpy_src / "lib" / "tests").mkdir()
    (numpy_src / "lib" / "tests" / "__init__.py").write_text("", encoding="utf-8")
    (numpy_src / "lib" / "tests" / "test_impl.py").write_text(
        "SHOULD_NOT_BE_STAGED = True\n", encoding="utf-8"
    )
    # NumPy's own version generator + pyproject.
    (repo / closure._NUMPY_SOURCE_REL / "pyproject.toml").write_text(
        '[project]\nversion = "5.5.5"\n', encoding="utf-8"
    )
    (numpy_src / "_build_utils" / "gitversion.py").write_text(
        _FAKE_GITVERSION, encoding="utf-8"
    )
    # Real meson build output carrying __config__.py.
    build_cfg = (
        repo
        / "tmp/pact_numpy_multiarray_meson_wasm_build_generated/numpy/__config__.py"
    )
    build_cfg.parent.mkdir(parents=True)
    build_cfg.write_text(_FAKE_CONFIG, encoding="utf-8")
    # A sealed root that owns only the C-ext-adjacent files.
    seal_numpy = repo / "tmp/pact_numpy_multiarray_sealed_for_witness/numpy"
    (seal_numpy / "_core").mkdir(parents=True)
    (seal_numpy / "__init__.py").write_text("PARTIAL = 1\n", encoding="utf-8")
    (seal_numpy / "_core" / "__init__.py").write_text("", encoding="utf-8")
    (seal_numpy / "_core" / "_multiarray_umath.molt.wasm").write_bytes(b"\0asm")
    return repo


def test_stage_mirrors_full_pure_python_subtree_into_seal(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    seal_numpy = repo / "tmp/pact_numpy_multiarray_sealed_for_witness/numpy"

    # Before: the partial-package submodules are absent -> would fail-close.
    assert not (seal_numpy / "_globals.py").exists()
    assert not (seal_numpy / "_expired_attrs_2_0.py").exists()

    closure.stage(repo)

    # Every importable submodule is now present in the seal.
    assert (seal_numpy / "_globals.py").read_text(encoding="utf-8") == "_NoValue = object()\n"
    assert (seal_numpy / "_expired_attrs_2_0.py").is_file()
    assert (seal_numpy / "lib" / "__init__.py").is_file()
    assert (seal_numpy / "lib" / "_impl.py").is_file()
    # Generated modules materialized on top.
    assert 'version = "5.5.5"' in (seal_numpy / "version.py").read_text(encoding="utf-8")
    assert (seal_numpy / "__config__.py").is_file()


def test_stage_excludes_tests_and_preserves_c_extension(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    seal_numpy = repo / "tmp/pact_numpy_multiarray_sealed_for_witness/numpy"
    closure.stage(repo)
    # unit-test trees are never imported and must not be staged.
    assert not (seal_numpy / "lib" / "tests" / "test_impl.py").exists()
    # the compiled extension the seal owns is untouched.
    assert (seal_numpy / "_core" / "_multiarray_umath.molt.wasm").read_bytes() == b"\0asm"


def test_check_flags_partial_package_then_passes_after_stage(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    problems = closure.check(repo)
    assert problems and any("missing staged module" in p for p in problems)

    closure.stage(repo)
    assert closure.check(repo) == []


def test_check_detects_source_drift(tmp_path: Path) -> None:
    repo = _make_repo(tmp_path)
    closure.stage(repo)
    # Diverge a staged module from source: check must flag drift.
    seal_globals = (
        repo / "tmp/pact_numpy_multiarray_sealed_for_witness/numpy/_globals.py"
    )
    seal_globals.write_text("_NoValue = None  # drifted\n", encoding="utf-8")
    problems = closure.check(repo)
    assert any("stale staged module" in p for p in problems)


def test_missing_numpy_source_fails_closed(tmp_path: Path) -> None:
    repo = tmp_path / "empty"
    repo.mkdir()
    with pytest.raises(closure.ClosureStagingError):
        closure.stage(repo)


def test_generated_modules_are_not_clobbered_by_source_mirror(tmp_path: Path) -> None:
    # version.py/__config__.py are protected names: even if the source tree had
    # them, the mirror must defer to the generator authority.
    repo = _make_repo(tmp_path)
    numpy_src = repo / closure._NUMPY_SOURCE_REL / "numpy"
    (numpy_src / "version.py").write_text("version = 'BOGUS'\n", encoding="utf-8")
    closure.stage(repo)
    seal_version = (
        repo / "tmp/pact_numpy_multiarray_sealed_for_witness/numpy/version.py"
    )
    text = seal_version.read_text(encoding="utf-8")
    assert "BOGUS" not in text
    assert 'version = "5.5.5"' in text
    _ = generated  # generator authority exercised via closure.stage

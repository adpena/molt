"""Teeth for the cross-language duplicate-authority drift gate.

`tools/check_table_drift.py` binds hand-synced tables where a one-value drift is
a silent miscompile. These tests are the gate's own anti-regression proof:

  * the gate is GREEN on the current tree (every bound table in sync), and
  * MUTATING a single value in any bound copy makes the corresponding category
    FAIL, and reverting restores PASS.

If a future change weakens the gate (stops parsing a copy, drops a category,
loosens an equality), the mutation half of these tests goes green-when-it-should-
be-red and this suite fails. The tables checked:

  #1 type-tags         frontend BUILTIN_TYPE_TAGS <-> runtime TYPE_TAG_*/BUILTIN_TAG_*
  #4 target-python      cli authority <-> stdlib-union baseline <-> generator

Run:
    uv run --python 3.12 pytest tests/test_table_drift_gate.py -q
"""

from __future__ import annotations
import sys
import tempfile
from pathlib import Path

import pytest

from tools import check_table_drift as GATE


ROOT = Path(__file__).resolve().parents[1]


def _category_ok(key: str) -> bool:
    return GATE.CATEGORIES[key]().ok


def test_gate_module_paths_resolve_to_this_repo() -> None:
    # A misresolved ROOT would make every check trivially green; pin it.
    assert GATE.ROOT == ROOT
    assert GATE.FRONTEND_TYPES_PY.exists()
    assert GATE.TYPE_IDS_RS.exists()
    assert Path(GATE.target_python_policy.__file__).resolve() == (
        ROOT / "src/molt/target_python_policy.py"
    )
    assert GATE.STDLIB_UNION_PY.exists()
    assert GATE.GEN_STDLIB_UNION_PY.exists()


@pytest.mark.parametrize(
    "key",
    sorted(["type-tags", "target-python"]),
)
def test_gate_green_on_current_tree(key: str) -> None:
    result = GATE.CATEGORIES[key]()
    failures = [f"{it.name}: {it.detail}" for it in result.items if not it.ok]
    assert result.ok, f"category {key} drifted on clean tree:\n" + "\n".join(failures)


class _Mutation:
    """Point the gate at a copy of one source file with one substring replaced.

    The copy lives in a temporary directory and the gate's path constant is
    redirected to it for the duration; the repository file is never written,
    because every test and proof command running beside this one reads it.
    Operates on raw bytes so line endings match the checked-out file.
    """

    def __init__(self, attribute: str, old: str, new: str) -> None:
        self.attribute = attribute
        self.path: Path = getattr(GATE, attribute)
        self._old = old
        self._new = new
        self._scratch: tempfile.TemporaryDirectory[str] | None = None

    def __enter__(self) -> "_Mutation":
        data = self.path.read_bytes()
        # Match the file's actual line ending so multi-line anchors work on
        # both LF and CRLF checkouts.
        newline = b"\r\n" if b"\r\n" in data else b"\n"
        old = self._old.encode("utf-8").replace(b"\n", newline)
        new = self._new.encode("utf-8").replace(b"\n", newline)
        assert old in data, f"mutation anchor not found in {self.path}: {old!r}"
        self._scratch = tempfile.TemporaryDirectory()
        mutated = Path(self._scratch.name) / self.path.name
        mutated.write_bytes(data.replace(old, new, 1))
        setattr(GATE, self.attribute, mutated)
        return self

    def __exit__(self, *exc: object) -> None:
        setattr(GATE, self.attribute, self.path)
        if self._scratch is not None:
            self._scratch.cleanup()


def _assert_mutation_fails(category: str, mutation: _Mutation) -> None:
    assert _category_ok(category), (
        f"precondition: {category} must be green before mutation"
    )
    with mutation:
        assert not _category_ok(category), (
            f"MUTATION NOT CAUGHT: {category} stayed green after mutating "
            f"{mutation.path.name} ({mutation._old!r} -> {mutation._new!r}). "
            "The drift gate has a hole."
        )
    assert _category_ok(category), f"{category} must be green again after revert"


def test_mutation_type_tag_int_value_caught() -> None:
    # #1 frontend side: change the int for `int` (1 -> 99).
    _assert_mutation_fails(
        "type-tags",
        _Mutation("FRONTEND_TYPES_PY", '    "int": 1,', '    "int": 99,'),
    )


def test_mutation_type_tag_runtime_const_caught() -> None:
    # #1 runtime side: change TYPE_TAG_INT (1 -> 42).
    _assert_mutation_fails(
        "type-tags",
        _Mutation(
            "TYPE_IDS_RS",
            "const TYPE_TAG_INT: i64 = 1;",
            "const TYPE_TAG_INT: i64 = 42;",
        ),
    )


@pytest.mark.parametrize(
    "text",
    [
        "// const TYPE_TAG_INT: i64 = 99;\nconst TYPE_TAG_INT: i64 = 2;",
        'const DECOY: &str = r#"const TYPE_TAG_INT: i64 = 99;"#;\nconst TYPE_TAG_INT: i64 = 2;',
        "const TYPE_TAG_INT: i64 = 2;\n/* const TYPE_TAG_INT: i64 = 99; */",
    ],
)
def test_rust_constant_projection_ignores_comment_and_literal_decoys(text: str) -> None:
    assert GATE._parse_rust_i64_consts(text) == {"TYPE_TAG_INT": 2}


def test_mutation_target_python_baseline_bump_caught() -> None:
    # #4 bump the stdlib-union baseline tuple only.
    _assert_mutation_fails(
        "target-python",
        _Mutation(
            "STDLIB_UNION_PY",
            'BASELINE_PYTHON_VERSIONS = ("3.12", "3.13", "3.14")',
            'BASELINE_PYTHON_VERSIONS = ("3.12", "3.13", "3.15")',
        ),
    )


def test_mutation_target_python_generator_redeclare_caught() -> None:
    # #4 re-declare DEFAULT_PYTHONS as a literal tuple (the drift signal).
    _assert_mutation_fails(
        "target-python",
        _Mutation(
            "GEN_STDLIB_UNION_PY",
            "DEFAULT_PYTHONS = SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS",
            'DEFAULT_PYTHONS = ("3.12", "3.13", "3.14")',
        ),
    )


def test_unicode_facts_gate_distinguishes_same_minor_across_majors(
    tmp_path, monkeypatch
):
    # This private catalog checks identity transport only, not version admission.
    monkeypatch.setattr(GATE, "ROOT", tmp_path)
    monkeypatch.setattr(
        GATE.target_python_policy,
        "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS",
        ("3.0", "4.0"),
    )
    fixtures = {
        "STDLIB_UNION_PY": 'BASELINE_PYTHON_VERSIONS = ("3.0", "4.0")\n',
        "GEN_STDLIB_UNION_PY": "DEFAULT_PYTHONS = SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS\n",
    }
    for attribute, source in fixtures.items():
        path = tmp_path / (attribute + ".py")
        path.write_text(source, encoding="utf-8")
        monkeypatch.setattr(GATE, attribute, path)
    pins = tmp_path / "config/cpython_regrtest_sources.toml"
    pins.parent.mkdir()
    pins.write_text(
        'schema = "molt.cpython-regrtest-sources.v1"\n'
        + "\n".join(
            f'[[source]]\npython = "{version}"\ntag = "v{version}.0"\n'
            f'revision = "{"a" * 40}"\n'
            'git_url = "https://github.com/python/cpython.git"\n'
            for version in ("3.0", "4.0")
        ),
        encoding="utf-8",
    )
    width = tmp_path / "runtime/molt-obj-model/src/unicode_generated.rs"
    width.parent.mkdir(parents=True)
    source = "(3, 0) => Some(&DATA_3_0),\n(4, 0) => Some(&DATA_4_0),\n"
    width.write_text(source, encoding="utf-8")
    assert GATE.check_target_python_versions().ok
    # Same minor, wrong major must not be reported as an identical target set.
    width.write_text(source.replace("(4, 0)", "(3, 0)"), encoding="utf-8")
    result = GATE.check_target_python_versions()
    assert not result.ok
    assert any(
        item.name == "unicode-facts-source-versions" and not item.ok
        for item in result.items
    )
    width.write_text(source, encoding="utf-8")
    assert GATE.check_target_python_versions().ok


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))

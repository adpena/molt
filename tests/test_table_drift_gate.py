"""Teeth for the cross-language duplicate-authority drift gate.

`tools/check_table_drift.py` binds hand-synced tables where a one-value drift is
a silent miscompile. These tests are the gate's own anti-regression proof:

  * the gate is GREEN on the current tree (all four tables in sync), and
  * MUTATING a single value in any bound copy makes the corresponding category
    FAIL, and reverting restores PASS.

If a future change weakens the gate (stops parsing a copy, drops a category,
loosens an equality), the mutation half of these tests goes green-when-it-should-
be-red and this suite fails. The tables checked:

  #1 type-tags         frontend BUILTIN_TYPE_TAGS <-> runtime TYPE_TAG_*/BUILTIN_TAG_*
  #2 exception-ordinals frontend enumerate() <-> runtime builtin_exception_name_for_tag
  #3 exception-names    frontend BUILTIN_EXCEPTION_NAMES <-> runtime exception_base_spec
  #4 target-python      cli authority <-> stdlib-union baseline <-> generator

Run:
    uv run --python 3.12 pytest tests/test_table_drift_gate.py -q
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest


def _find_repo_root() -> Path:
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        if out:
            return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()
GATE_PATH = ROOT / "tools" / "check_table_drift.py"


def _load_gate():
    spec = importlib.util.spec_from_file_location("check_table_drift", GATE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Register before exec so @dataclass introspection
    # (sys.modules.get(cls.__module__)) can resolve the module's namespace.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


GATE = _load_gate()


def _category_ok(key: str) -> bool:
    return GATE.CATEGORIES[key]().ok


def test_gate_module_paths_resolve_to_this_repo() -> None:
    # A misresolved ROOT would make every check trivially green; pin it.
    assert GATE.ROOT == ROOT
    assert GATE.FRONTEND_TYPES_PY.exists()
    assert GATE.TYPE_IDS_RS.exists()
    assert GATE.EXCEPTIONS_RS.exists()
    assert GATE.TARGET_PYTHON_PY.exists()
    assert GATE.STDLIB_UNION_PY.exists()
    assert GATE.GEN_STDLIB_UNION_PY.exists()


@pytest.mark.parametrize(
    "key",
    sorted(["type-tags", "exception-ordinals", "exception-names", "target-python"]),
)
def test_gate_green_on_current_tree(key: str) -> None:
    result = GATE.CATEGORIES[key]()
    failures = [f"{it.name}: {it.detail}" for it in result.items if not it.ok]
    assert result.ok, f"category {key} drifted on clean tree:\n" + "\n".join(failures)


class _Mutation:
    """Temporarily replace one substring in a source file, restore on exit.

    Operates on raw bytes so the restored file is byte-identical to the
    original (no platform newline translation), keeping the working tree
    clean after the test regardless of the checked-out line endings.
    """

    def __init__(self, path: Path, old: str, new: str) -> None:
        self.path = path
        self._old = old
        self._new = new
        self._original: bytes | None = None

    def __enter__(self) -> "_Mutation":
        data = self.path.read_bytes()
        # Match the file's actual line ending so multi-line anchors work on
        # both LF and CRLF checkouts.
        newline = b"\r\n" if b"\r\n" in data else b"\n"
        old = self._old.encode("utf-8").replace(b"\n", newline)
        new = self._new.encode("utf-8").replace(b"\n", newline)
        assert old in data, f"mutation anchor not found in {self.path}: {old!r}"
        self._original = data
        self.path.write_bytes(data.replace(old, new, 1))
        return self

    def __exit__(self, *exc: object) -> None:
        if self._original is not None:
            self.path.write_bytes(self._original)


@pytest.fixture(autouse=True)
def _reload_gate_after_mutation() -> Iterator[None]:
    """Each category re-reads files from disk, so no module reload is needed;
    but guard the tree is clean after every test regardless of assertion order."""
    yield


def _assert_mutation_fails(category: str, mutation: _Mutation) -> None:
    assert _category_ok(category), (
        f"precondition: {category} must be green before mutation"
    )
    with mutation:
        assert not _category_ok(category), (
            f"MUTATION NOT CAUGHT: {category} stayed green after mutating "
            f"{mutation.path.name} ({mutation.old!r} -> {mutation.new!r}). "
            "The drift gate has a hole."
        )
    assert _category_ok(category), f"{category} must be green again after revert"


def test_mutation_type_tag_int_value_caught() -> None:
    # #1 frontend side: change the int for `int` (1 -> 99).
    _assert_mutation_fails(
        "type-tags",
        _Mutation(GATE.FRONTEND_TYPES_PY, '    "int": 1,', '    "int": 99,'),
    )


def test_mutation_type_tag_runtime_const_caught() -> None:
    # #1 runtime side: change TYPE_TAG_INT (1 -> 42).
    _assert_mutation_fails(
        "type-tags",
        _Mutation(
            GATE.TYPE_IDS_RS,
            "const TYPE_TAG_INT: i64 = 1;",
            "const TYPE_TAG_INT: i64 = 42;",
        ),
    )


def test_mutation_exception_ordinal_shift_caught() -> None:
    # #2 frontend side: swap two names -> ordinals 3/4 flip on one side only.
    _assert_mutation_fails(
        "exception-ordinals",
        _Mutation(
            GATE.FRONTEND_TYPES_PY,
            '            "KeyError",\n            "IndexError",',
            '            "IndexError",\n            "KeyError",',
        ),
    )


def test_mutation_exception_ordinal_runtime_name_caught() -> None:
    # #2 runtime side: change the name at ordinal 3.
    _assert_mutation_fails(
        "exception-ordinals",
        _Mutation(
            GATE.EXCEPTIONS_RS,
            '        3 => Some("KeyError"),',
            '        3 => Some("ValueError"),',
        ),
    )


def test_mutation_exception_name_frontend_only_caught() -> None:
    # #3 add a name to the frontend set only.
    _assert_mutation_fails(
        "exception-names",
        _Mutation(
            GATE.FRONTEND_TYPES_PY,
            "BUILTIN_EXCEPTION_NAMES = {\n",
            'BUILTIN_EXCEPTION_NAMES = {\n    "TotallyFakeError",\n',
        ),
    )


def test_mutation_exception_name_runtime_only_caught() -> None:
    # #3 add a real builtin exception to the runtime spec only (frontend can't
    # name it) -- proves the runtime->frontend direction has teeth and is not
    # masked by the KNOWN_NON_FRONTEND allowlist.
    _assert_mutation_fails(
        "exception-names",
        _Mutation(
            GATE.EXCEPTIONS_RS,
            '        "ModuleNotFoundError" => Some(ExceptionBaseSpec::One("ImportError")),',
            '        "ModuleNotFoundError" | "TotallyFakeRuntimeError" => '
            'Some(ExceptionBaseSpec::One("ImportError")),',
        ),
    )


def test_mutation_target_python_baseline_bump_caught() -> None:
    # #4 bump the stdlib-union baseline tuple only.
    _assert_mutation_fails(
        "target-python",
        _Mutation(
            GATE.STDLIB_UNION_PY,
            'BASELINE_PYTHON_VERSIONS = ("3.12", "3.13", "3.14")',
            'BASELINE_PYTHON_VERSIONS = ("3.12", "3.13", "3.15")',
        ),
    )


def test_mutation_target_python_generator_redeclare_caught() -> None:
    # #4 re-declare DEFAULT_PYTHONS as a literal tuple (the drift signal).
    _assert_mutation_fails(
        "target-python",
        _Mutation(
            GATE.GEN_STDLIB_UNION_PY,
            "DEFAULT_PYTHONS = SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS",
            'DEFAULT_PYTHONS = ("3.12", "3.13", "3.14")',
        ),
    )


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))

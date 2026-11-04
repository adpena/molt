"""Runtime-name authority: synchronization and independent census refusals."""

from pathlib import Path

import pytest

from tools import gen_runtime_names as names


def test_runtime_name_projection_matches_registry() -> None:
    for path, expected in names.generated_outputs().items():
        assert path.read_text(encoding="utf-8") == expected


def test_runtime_name_census_requires_production_consumer(tmp_path: Path) -> None:
    root = tmp_path / "runtime/molt-runtime/src"
    root.mkdir(parents=True)
    (root / "lib.rs").write_text(
        "fn live() { NameId::Present; }\n"
        "#[cfg(test)] mod tests { fn test_only() { NameId::Dead; } }\n"
        '// NameId::Dead\nconst TEXT: &str = "NameId::Dead";\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="without consumers: Dead"):
        names.census({"Present": "present", "Dead": "dead"}, tmp_path)


def test_runtime_name_census_refuses_retired_spelled_lookup(tmp_path: Path) -> None:
    root = tmp_path / "runtime/molt-runtime/src"
    root.mkdir(parents=True)
    (root / "lib.rs").write_text(
        'fn live() { NameId::Present; runtime_static_name_slot(py, b"present"); }',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="byte-spelled static name lookup"):
        names.census({"Present": "present"}, tmp_path)


@pytest.mark.parametrize(
    "call",
    [
        'lookup_special_method(py, receiver.bits(), b"present")',
        'class_namespace_lookup_mro_pure(py, class, b"present", None, preparation)',
        'dynamic_attr_name_bits(py, b"present")',
        'attr_name_bits_from_bytes(py, b"present")',
        'pickle_attr_optional(py, object, b"present")',
        'NativeCallableSpec::declared(kind, owner, "present")',
        'native_method_table!(lookup, publish, py, name, [], {}, { "present" => None, })',
    ],
)
def test_runtime_name_census_refuses_migrated_spelled_consumers(
    tmp_path: Path, call: str
) -> None:
    root = tmp_path / "runtime/molt-runtime/src"
    root.mkdir(parents=True)
    (root / "lib.rs").write_text(
        "fn live() { NameId::Present; " + call + "; }", encoding="utf-8"
    )
    with pytest.raises(ValueError, match="byte-spelled runtime name lookup"):
        names.census({"Present": "present"}, tmp_path)

"""Freshness guard for the Rust-native StringPrep generated tables."""

from __future__ import annotations

from pathlib import Path

from tools import gen_stringprep_tables


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "runtime/molt-runtime-stringprep/src/tables.rs"


def test_generated_stringprep_tables_are_in_sync() -> None:
    gen = gen_stringprep_tables
    rendered = gen.generated_outputs()[OUT]
    checked_in = OUT.read_text(encoding="utf-8")

    assert checked_in == rendered, (
        f"{OUT.relative_to(ROOT)} is stale; run "
        "`python3 tools/gen_stringprep_tables.py --write`."
    )


def _records(cache: Path):
    from tools import scan_records

    return scan_records.ScanRecords(
        "generator_gen_stringprep_tables",
        owner=Path(gen_stringprep_tables.__file__),
        cache_root=cache,
    )


def test_stringprep_tables_replay_keyed_by_their_oracle(
    tmp_path: Path, monkeypatch
) -> None:
    import stringprep

    gen = gen_stringprep_tables
    computed = gen.generated_outputs()[OUT]
    cold = _records(tmp_path / "cache")
    assert gen.generated_outputs(scan_records=cold)[OUT] == computed
    assert cold.misses == 1 and cold.publish() is True
    warm = _records(tmp_path / "cache")
    assert gen.generated_outputs(scan_records=warm)[OUT] == computed
    assert (warm.hits, warm.misses) == (1, 0)

    # A different stringprep oracle source is a different key.
    edited = tmp_path / "stringprep.py"
    edited.write_bytes(Path(stringprep.__file__).read_bytes() + b"# edited\n")
    monkeypatch.setattr(stringprep, "__file__", str(edited))
    changed = _records(tmp_path / "cache")
    assert gen.generated_outputs(scan_records=changed)[OUT] == computed
    assert changed.misses == 1

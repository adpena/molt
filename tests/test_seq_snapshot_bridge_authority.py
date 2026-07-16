"""Keep satellite sequence snapshots feature-independent and single-authority."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CANONICAL = ROOT / "runtime/molt-runtime/src/seq_snapshot_bridge.rs"
OPTIONAL_BRIDGES = (ROOT / "runtime/molt-runtime/src/itertools_bridge.rs",)


def test_sequence_snapshot_symbol_has_one_unconditional_authority() -> None:
    canonical = CANONICAL.read_text(encoding="utf-8")
    assert canonical.count('pub extern "C" fn molt_seq_snapshot(') == 1

    for bridge in OPTIONAL_BRIDGES:
        source = bridge.read_text(encoding="utf-8")
        assert 'pub extern "C" fn molt_seq_snapshot(' not in source
        assert "seq_snapshot_bridge::export" not in source

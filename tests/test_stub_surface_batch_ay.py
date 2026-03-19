from __future__ import annotations

from pathlib import Path


def test_oem_hides_raw_capability_intrinsic() -> None:
    path = (
        Path(__file__).resolve().parents[1]
        / "src/molt/stdlib/encodings/oem.py"
    )
    source = path.read_text()
    assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
    assert '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")' in source

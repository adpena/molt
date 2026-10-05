from pathlib import Path

import pytest

from tools.proof_queue_pkg import cargo_output_layout


@pytest.fixture
def cargo_output_implementation_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    """Model source and outputs as siblings even with an in-checkout basetemp."""
    source = tmp_path / "implementation-source"
    source.mkdir()
    monkeypatch.setattr(
        cargo_output_layout, "implementation_source_root", lambda: source
    )
    return source

"""Teeth for the gate-liveness canary (A8's 'no gate without a canary').

Every hook's known-bad fixture must still FIRE; if a gate silently goes inert
the canary fails -- proven here both green (all live) and red (a rotted gate).
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.check_gate_liveness as cgl  # noqa: E402
import tools.hooks.bash_guard as bg  # noqa: E402


def test_all_canaries_live():
    results = cgl.run()
    dead = [c.name for c, ok in results if not ok]
    assert not dead, f"dead canaries: {dead}"


def test_main_check_passes_when_healthy():
    assert cgl.main(["--check"]) == 0


def test_main_check_fails_when_a_gate_rots(monkeypatch):
    # Simulate bash_guard.decide silently going inert (always allows).
    monkeypatch.setattr(bg, "decide", lambda *a, **k: bg.Decision(block=False))
    assert cgl.main(["--check"]) == 1

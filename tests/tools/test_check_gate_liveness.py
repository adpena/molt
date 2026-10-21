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


def test_target_subset_canary_uses_live_admission_authority(monkeypatch):
    canary = next(
        item for item in cgl._canaries() if item.gate == "cpython_version_boundary"
    )
    assert canary.fires()
    admitted = []

    def admit_every_tuple(target, *, platform=None):
        admitted.append((target.short, platform))
        return target

    monkeypatch.setattr(cgl, "require_verified_subset_target", admit_every_tuple)
    assert not canary.fires()
    assert admitted == [("3.11", "windows")]


def test_main_check_passes_when_healthy():
    assert cgl.main(["--check"]) == 0


def test_main_check_fails_when_a_gate_rots(monkeypatch):
    # Simulate bash_guard.decide silently going inert (always allows).
    monkeypatch.setattr(bg, "decide", lambda *a, **k: bg.Decision(block=False))
    assert cgl.main(["--check"]) == 1


_HOOK_HARM = {"id": "h", "guard": "tools.hooks.guard", "canary": "guard.c"}


def _problems(harm, *, wired=True, state="strict", open_keys=("HF-1",)):
    judged, _wired = cgl.hook_modules([harm])
    return cgl.hook_enforcement_problems(
        [harm],
        judged=judged,
        wired={str(harm["guard"])} if wired else set(),
        flip_states={"guard": state},
        open_keys=set(open_keys),
    )


def test_an_enforcing_hook_guard_needs_no_gap_and_refuses_a_stale_one():
    assert _problems(_HOOK_HARM) == []
    assert _problems({**_HOOK_HARM, "gap": "HF-1"}) == [
        "h: guard enforces, so gap HF-1 is stale"
    ]


def test_an_unenforced_hook_guard_must_name_an_open_gap_row():
    assert _problems(_HOOK_HARM, wired=False) == [
        "h: guard is unwired and names no gap row"
    ]
    assert _problems(_HOOK_HARM, state="warn") == [
        "h: guard is observe-only and names no gap row"
    ]
    assert _problems({**_HOOK_HARM, "gap": "HF-1"}, state="warn") == []
    assert _problems({**_HOOK_HARM, "gap": "HF-2"}, wired=False) == [
        "h: gap HF-2 is not an open ledger row"
    ]


def test_guards_outside_the_hook_spine_are_not_judged_by_hook_wiring():
    harm = {"id": "h", "guard": "tools.disk_guard", "canary": "disk_guard.c"}
    assert _problems(harm, wired=False) == []


def test_live_wiring_reads_the_declared_hooks_and_the_stop_legs():
    from tools.hooks import stop_gates

    harms = [{"guard": "tools.hooks.session_learning"}]
    judged, wired = cgl.hook_modules(harms)
    assert {"tools.hooks.bash_guard", "tools.hooks.session_digest"} <= wired
    legs = {leg.evaluate.__module__ for leg in stop_gates.LEGS}
    # A Stop leg outside the hook package is judged too (anti-recurrence).
    assert "tools.anti_recurrence_gate" in legs and legs <= judged and legs <= wired
    assert "tools.hooks.session_learning" in judged
    assert "tools.hooks.session_learning" not in wired


def test_settings_json_is_the_projection_of_the_declared_wiring():
    from tools.hooks import wiring

    settings = (cgl.ROOT / ".claude" / "settings.json").read_text(encoding="utf-8")
    assert settings == wiring.settings_document()

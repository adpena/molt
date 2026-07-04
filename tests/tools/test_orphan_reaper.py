"""Regression tests for the ownership + PID-reuse gates in ``orphan_reaper``.

The reaper is the one custody net that historically selected kill targets purely
by dead-parent + build-tool name, with no tie to Molt ownership and no
snapshot-to-kill identity re-validation. That conflated "a build tool whose
launcher exited" with "a leaked *Molt* build tool" and could collateral-kill an
unrelated non-Molt build, a Windows-PID-reuse victim, or a console host. These
tests pin the fix: selection now requires ``process_sentinel.is_molt_process``,
and termination re-validates process identity (creation time) at the kill.
"""

from __future__ import annotations

import importlib.util
from functools import cache
from pathlib import Path
import signal
import sys

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "tools" / "orphan_reaper.py"
WINDOWS_ROOT = Path(r"C:\repo\molt")


@cache
def _load_orphan_reaper():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_orphan_reaper", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _force_windows_model(module, monkeypatch) -> None:
    # ``is_molt_process`` casefolds path matching only under the Windows process
    # model; force it so the ownership gate exercises real Windows command lines
    # regardless of the host the test runs on.
    monkeypatch.setattr(module.process_sentinel, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)


def _sample(module, *, pid, ppid, command, started_at_ns=1_000):
    return module.memory_guard.ProcessSample(
        pid=pid,
        ppid=ppid,
        rss_kb=1_000,
        command=command,
        pgid=None,
        elapsed_sec=10,
        started_at_ns=started_at_ns,
    )


def _molt_leaked_cargo_command() -> str:
    # A leaked Molt build tool: cargo running under the Molt repo's target root.
    return (
        r"cargo.exe build --package molt-backend "
        r"--target-dir C:\repo\molt\target\debug"
    )


def _unrelated_cargo_command() -> str:
    # An unrelated non-Molt Rust build whose launcher shell already exited.
    return (
        r"cargo.exe build --release "
        r"--manifest-path C:\Users\someone\other-rust-proj\Cargo.toml"
    )


def test_non_molt_dead_parent_build_tool_not_selected(monkeypatch) -> None:
    """The defect: a dead-parent REAP_NAMES process that is NOT Molt-owned.

    Against the pre-fix reaper this was selected purely by name + dead parent.
    Post-fix it must NOT be selected — it fails the Molt-ownership gate.
    """
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    samples = {
        4321: _sample(
            module,
            pid=4321,
            ppid=999,  # launcher shell already exited -> not in ``samples`` -> orphaned
            command=_unrelated_cargo_command(),
        ),
    }

    orphans = module.find_orphans(samples, root=WINDOWS_ROOT, self_pid=1)

    assert orphans == [], (
        "an unrelated non-Molt dead-parent build tool must NOT be reaped; "
        f"got {[s.command for s in orphans]!r}"
    )


def test_molt_owned_leaked_build_tool_still_selected(monkeypatch) -> None:
    """The tool must still net a genuinely-Molt-owned leaked build subtree."""
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    samples = {
        4321: _sample(
            module,
            pid=4321,
            ppid=999,  # dead launcher -> orphaned
            command=_molt_leaked_cargo_command(),
        ),
    }

    orphans = module.find_orphans(samples, root=WINDOWS_ROOT, self_pid=1)

    assert [s.pid for s in orphans] == [4321], (
        "a Molt-owned leaked build tool with a dead parent MUST still be reaped; "
        f"got {[s.command for s in orphans]!r}"
    )


def test_both_present_only_molt_owned_selected(monkeypatch) -> None:
    """Side-by-side: only the Molt-owned orphan is selected."""
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    samples = {
        100: _sample(
            module, pid=100, ppid=999, command=_molt_leaked_cargo_command()
        ),
        200: _sample(
            module, pid=200, ppid=888, command=_unrelated_cargo_command()
        ),
    }

    orphans = module.find_orphans(samples, root=WINDOWS_ROOT, self_pid=1)

    assert [s.pid for s in orphans] == [100]


def test_molt_owned_but_parent_alive_not_selected(monkeypatch) -> None:
    """An active Molt build (parent alive) is never reaped."""
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    samples = {
        50: _sample(
            module,
            pid=50,
            ppid=1,
            command=r"C:\repo\molt\target\debug\deps\build-script.exe",
        ),
        100: _sample(
            module,
            pid=100,
            ppid=50,  # parent 50 is alive in ``samples`` -> not orphaned
            command=_molt_leaked_cargo_command(),
        ),
    }

    orphans = module.find_orphans(samples, root=WINDOWS_ROOT, self_pid=1)

    assert orphans == []


def test_conhost_dropped_from_reap_names() -> None:
    """A console host is almost never a leaked Molt build tool; it backs live
    consoles. It must not be a name-matched reap target."""
    module = _load_orphan_reaper()
    assert "conhost.exe" not in module.REAP_NAMES


def test_reap_one_refuses_on_pid_reuse_identity_mismatch(monkeypatch) -> None:
    """PID-reuse gate: if the PID's identity changed between snapshot and kill,
    ``_reap_one`` must NOT deliver a signal (fail closed)."""
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    snapshot_sample = _sample(
        module,
        pid=4321,
        ppid=999,
        command=_molt_leaked_cargo_command(),
        started_at_ns=1_000,
    )
    # At kill time the PID has been recycled to a different process: same PID,
    # DIFFERENT creation time (started_at_ns) -> different process_identity.
    recycled_sample = _sample(
        module,
        pid=4321,
        ppid=1,
        command=r"C:\Windows\System32\notepad.exe",
        started_at_ns=9_999,
    )

    sent: list[tuple[int, int]] = []

    def fake_os_kill(pid, sig):
        sent.append((pid, sig))

    # Route the shared identity-checked primitive at the recycled table and
    # observe whether it would signal. ``os.kill`` lives in the custody module.
    monkeypatch.setattr(
        module.memory_guard, "sample_processes_windows_hard_timeout",
        lambda: {4321: recycled_sample},
    )
    monkeypatch.setattr(
        module.memory_guard._process_custody.os, "kill", fake_os_kill
    )

    killed = module._reap_one(snapshot_sample, grace=0.0)

    assert killed is False
    assert sent == [], (
        "a recycled PID (identity mismatch) must never be signalled; "
        f"got signals {sent!r}"
    )


def test_reap_one_signals_when_identity_matches(monkeypatch) -> None:
    """Positive control: a stable identity is signalled (SIGTERM then fallback)."""
    module = _load_orphan_reaper()
    _force_windows_model(module, monkeypatch)

    sample = _sample(
        module,
        pid=4321,
        ppid=999,
        command=_molt_leaked_cargo_command(),
        started_at_ns=1_000,
    )

    sent: list[tuple[int, int]] = []

    def fake_os_kill(pid, sig):
        sent.append((pid, sig))

    monkeypatch.setattr(
        module.memory_guard, "sample_processes_windows_hard_timeout",
        lambda: {4321: sample},
    )
    monkeypatch.setattr(
        module.memory_guard._process_custody.os, "kill", fake_os_kill
    )

    killed = module._reap_one(sample, grace=0.0)

    assert killed is True
    assert sent, "a stable Molt-owned identity must be signalled"
    assert sent[0] == (4321, int(signal.SIGTERM))

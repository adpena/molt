from __future__ import annotations

import zipfile
from pathlib import Path

import pytest

from tools import trunk_loop

CELLS = ("portability.queue.linux",)


def _member(name: str, size: int = 1) -> zipfile.ZipInfo:
    item = zipfile.ZipInfo(name)
    item.file_size = size
    return item


def test_cell_results_may_nest_receipt_detail():
    # The proof executor writes guard profiles under `<receipt>.d/`; postsubmit
    # collection refused them on 2026-11-07 because non-landing runs were held
    # to exactly `cell/file`.
    for name in (
        "portability.queue.linux/receipt.json",
        "portability.queue.linux/receipt.d/"
        "portability.queue.linux.attempt-1.profile.jsonl",
    ):
        path = trunk_loop.admitted_result_member(_member(name), CELLS)
        assert path.parts[0] == "portability.queue.linux"


@pytest.mark.parametrize(
    "name, size",
    [
        ("portability.queue.linux", 1),  # no file below the cell
        ("/portability.queue.linux/receipt.json", 1),
        ("portability.queue.linux/../escape.json", 1),
        ("portability.queue.linux\\..\\escape.json", 1),
        ("other.cell/receipt.json", 1),
        ("portability.queue.linux/" + "/".join(["d"] * 16) + "/deep.json", 1),
        ("portability.queue.linux/receipt.json", 512 * 1024 * 1024 + 1),
    ],
)
def test_result_members_stay_inside_a_selected_cell(name, size):
    with pytest.raises(ValueError, match="unexpected result archive member"):
        trunk_loop.admitted_result_member(_member(name, size), CELLS)


def test_declared_evidence_rides_in_the_cell_stage(tmp_path: Path) -> None:
    """The run checkout is retired; the stage archive carries its evidence."""
    checkout, stage = (
        tmp_path / "checkout",
        tmp_path / "stage" / "performance.instructions",
    )
    evidence = checkout / "proof-receipts/evidence/instructions"
    (evidence / "programs").mkdir(parents=True)
    (evidence / "verdict.json").write_text("{}", encoding="utf-8")
    (evidence / "programs/loop.py").write_text("print(1)\n", encoding="utf-8")
    stage.mkdir(parents=True)
    receipt = {
        "commands": [
            {"evidence_outputs": [{"path": "proof-receipts/evidence/instructions"}]},
            {"evidence_outputs": []},
        ]
    }
    trunk_loop.copy_declared_evidence(receipt, checkout, stage)
    copied = stage / "evidence/proof-receipts/evidence/instructions"
    assert (copied / "verdict.json").read_text(encoding="utf-8") == "{}"
    assert (copied / "programs/loop.py").is_file()
    member = "performance.instructions/evidence/proof-receipts/evidence/instructions/verdict.json"
    assert (
        trunk_loop.admitted_result_member(
            _member(member), ("performance.instructions",)
        ).name
        == "verdict.json"
    )
    with pytest.raises(ValueError):
        trunk_loop.copy_declared_evidence(
            {"commands": [{"evidence_outputs": [{"path": "../escape"}]}]},
            checkout,
            stage,
        )


class _Probe:
    def __init__(self, answer: str) -> None:
        self.answer, self.scripts = answer, []

    def shell_command(self, script: str, *, timeout: int = 60) -> str:
        self.scripts.append(script)
        return self.answer


def test_host_busy_reads_the_worker_lock_with_the_maintenance_probe() -> None:
    host = type("Host", (), {"remote_dir": "/home/u/Molt/tmp/trunk-loop"})()
    for answer, busy in (("busy", True), ("idle", False)):
        probe = _Probe(answer)
        assert trunk_loop.host_busy(host, probe) is busy
        assert "/home/u/Molt/tmp/trunk-loop/trunk-worker.lock" in probe.scripts[0]
        assert 'flock -n "$lock" true' in probe.scripts[0]


def test_if_idle_stages_nothing_while_the_host_is_busy(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def staged(*args: object, **kwargs: object) -> None:
        raise AssertionError("a busy host must not be staged or retired")

    monkeypatch.setattr(trunk_loop, "host_busy", lambda host, transport: True)
    monkeypatch.setattr(trunk_loop, "start", staged)
    monkeypatch.setattr(trunk_loop, "retire_terminal_runs", staged)
    argv = ["run", "--host", "molt-gate", "--commit", "HEAD"]
    argv += ["--cell", "performance.instructions", "--if-idle"]
    assert trunk_loop.main(argv) == 0
    assert "busy; nothing staged, not measured" in capsys.readouterr().out

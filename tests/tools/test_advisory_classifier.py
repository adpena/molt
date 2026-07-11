import json
import sys
from pathlib import Path
import tools.advisory_classifier as advisory


def test_closed_enum_has_teeth():
    assert (
        advisory.decide_output("poison", text="x", schema=("poison", "benign")).verdict
        == "poison"
    )
    assert (
        advisory.decide_output(
            "poison because maybe", text="x", schema=("poison", "benign")
        ).verdict
        is None
    )


def test_prompt_echo_is_rejected():
    assert (
        advisory.decide_output(
            "source text", text="source text", schema=("benign",)
        ).reason
        == "prompt-echo"
    )


def test_subprocess_and_durable_event(tmp_path: Path):
    script = tmp_path / "model.py"
    script.write_text("print('dismissal')\n", encoding="utf-8")
    log = tmp_path / "events.jsonl"
    assert (
        advisory.classify(
            "too small",
            ("dismissal", "unclear"),
            command=f'"{sys.executable}" "{script}"',
            event_log=log,
        )
        == "dismissal"
    )
    assert json.loads(log.read_text(encoding="utf-8"))["verdict"] == "dismissal"


def test_raising_backend_fails_open(tmp_path: Path):
    assert (
        advisory.classify(
            "x",
            ("poison",),
            command="not-a-real-command-7fcd9",
            event_log=tmp_path / "events.jsonl",
        )
        is None
    )

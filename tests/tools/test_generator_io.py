from __future__ import annotations

from pathlib import Path

import pytest

from tools import generator_io


def _render(tmp_path: Path, text: str = "fresh\n"):
    return lambda: {tmp_path / "out" / "a.txt": text, tmp_path / "b.txt": "b\n"}


def test_write_publishes_stale_outputs_and_check_then_passes(tmp_path, capsys):
    render = _render(tmp_path)
    assert generator_io.generator_main(render, ["--check"]) == 1
    assert "stale:" in capsys.readouterr().err
    assert generator_io.generator_main(render, ["--write"]) == 0
    assert (tmp_path / "out" / "a.txt").read_bytes() == b"fresh\n"
    assert generator_io.generator_main(render, ["--check"]) == 0
    # A current output is never rewritten.
    assert generator_io.write_outputs(render()) == []


def test_check_ignores_checkout_newline_policy(tmp_path):
    render = _render(tmp_path)
    generator_io.generator_main(render, ["--write"])
    (tmp_path / "b.txt").write_bytes(b"b\r\n")
    assert generator_io.stale_outputs(render()) == []


@pytest.mark.parametrize("argv", [[], ["--check", "--write"]])
def test_exactly_one_mode_is_required(tmp_path, argv):
    with pytest.raises(SystemExit) as raised:
        generator_io.generator_main(_render(tmp_path), argv)
    assert raised.value.code == 2


def test_rustfmt_source_formats_through_the_shared_generator_guard(monkeypatch):
    from types import SimpleNamespace

    from tools import harness_memory_guard

    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(returncode=0, stdout="fn main() {}\n", stderr="")

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    assert generator_io.rustfmt_source("fn main(){}\n", label="t") == "fn main() {}\n"
    assert calls == [
        {
            "cmd": ["rustfmt", "--edition", "2024", "--emit", "stdout"],
            "prefix": "MOLT_GENERATOR",
            "cwd": generator_io.ROOT,
            "input": "fn main(){}\n",
            "capture_output": True,
            "text": True,
            "timeout": 60.0,
        }
    ]


def test_rustfmt_source_names_the_output_when_rustfmt_fails(monkeypatch):
    from types import SimpleNamespace

    from tools import harness_memory_guard

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        lambda cmd, **kwargs: SimpleNamespace(returncode=1, stdout="", stderr="bad"),
    )
    with pytest.raises(RuntimeError, match="rustfmt failed for codec tables:\nbad"):
        generator_io.rustfmt_source("fn", label="codec tables")

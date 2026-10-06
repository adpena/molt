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

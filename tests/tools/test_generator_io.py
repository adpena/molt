from __future__ import annotations

import tempfile
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


def test_ruff_format_source_formats_an_excluded_generated_path():
    # Generated outputs are excluded from repository formatting runs; the
    # generator still formats them under the project configuration.
    from tools import gen_protocol

    assert (
        generator_io.ruff_format_source("value=1\n", path=gen_protocol.OUT_PROTOCOL)
        == "value = 1\n"
    )


def test_ruff_format_source_names_the_output_when_ruff_fails():
    path = generator_io.ROOT / "src" / "molt" / "_generated_probe.py"
    with pytest.raises(
        RuntimeError, match="ruff format failed for src/molt/_generated_probe.py"
    ):
        generator_io.ruff_format_source("def broken(:\n", path=path)


def _isolated_rustfmt_cache(monkeypatch, tmp_path):
    monkeypatch.setattr(
        generator_io,
        "_rustfmt_cache_path",
        lambda command, version, source: (
            tmp_path / "cache" / f"{abs(hash((tuple(command), version, source)))}.rs"
        ),
    )


def test_rustfmt_sources_formats_each_source_alone(monkeypatch, tmp_path):
    # A generated module declares children the generator writes separately;
    # rustfmt formats the file it names without resolving them.
    _isolated_rustfmt_cache(monkeypatch, tmp_path)
    formatted = generator_io.rustfmt_sources(
        {"mod.rs": "mod child;\npub fn f( )->u32{1}\n", "child.rs": "fn child(){}\n"},
        label="probe modules",
    )
    assert formatted == {
        "mod.rs": "mod child;\npub fn f() -> u32 {\n    1\n}\n",
        "child.rs": "fn child() {}\n",
    }


def test_rustfmt_sources_run_the_workspace_command_in_bounded_batches(
    monkeypatch, tmp_path
):
    from types import SimpleNamespace

    from molt.cargo_workspace import workspace_edition
    from tools import harness_memory_guard

    _isolated_rustfmt_cache(monkeypatch, tmp_path)
    monkeypatch.setattr(generator_io, "rustfmt_version", lambda: "rustfmt-test")
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        for path in cmd[5:]:
            Path(path).write_text("// formatted\n", encoding="utf-8")
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    # Room for two scratch paths per rustfmt run.
    scratch_path_cost = len(str(Path(tempfile.gettempdir()))) + 60
    monkeypatch.setattr(
        generator_io, "_RUSTFMT_ARGV_CHAR_BUDGET", 2 * scratch_path_cost
    )
    sources = {index: f"fn f{index}() {{}}\n" for index in range(5)}

    formatted = generator_io.rustfmt_sources(sources, label="probe")

    assert formatted == {index: "// formatted\n" for index in sources}
    command = ["rustfmt", "--edition", workspace_edition(generator_io.ROOT)]
    assert all(call["cmd"][:3] == command for call in calls)
    assert all(call["cmd"][3:5] == ["--config", "skip_children=true"] for call in calls)
    assert all(call["prefix"] == "MOLT_GENERATOR" for call in calls)
    scratch = [Path(path) for call in calls for path in call["cmd"][5:]]
    assert len(calls) > 1, "the command-line budget splits the batch"
    assert len(scratch) == len(sources)
    # One scratch directory per source, none in the checkout.
    assert len({path.parent for path in scratch}) == len(sources)
    assert not any(generator_io.ROOT in path.parents for path in scratch)


def test_rustfmt_sources_reuse_a_cached_format_without_running_rustfmt(
    monkeypatch, tmp_path
):
    from types import SimpleNamespace

    from tools import harness_memory_guard

    _isolated_rustfmt_cache(monkeypatch, tmp_path)
    monkeypatch.setattr(generator_io, "rustfmt_version", lambda: "rustfmt-test")
    runs = []

    def fake_guarded_completed_process(cmd, **kwargs):
        runs.append(cmd)
        for path in cmd[5:]:
            Path(path).write_text("fn f() {}\n", encoding="utf-8")
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    first = generator_io.rustfmt_source("fn f(){}\n", label="t")
    second = generator_io.rustfmt_source("fn f(){}\n", label="t")
    assert first == second == "fn f() {}\n"
    assert len(runs) == 1, "an unchanged source must not run rustfmt again"


def test_rustfmt_source_names_the_output_when_rustfmt_fails(monkeypatch, tmp_path):
    from types import SimpleNamespace

    from tools import harness_memory_guard

    _isolated_rustfmt_cache(monkeypatch, tmp_path)
    monkeypatch.setattr(generator_io, "rustfmt_version", lambda: "rustfmt-test")
    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        lambda cmd, **kwargs: SimpleNamespace(returncode=1, stdout="", stderr="bad"),
    )
    with pytest.raises(RuntimeError, match="rustfmt failed for codec tables:\nbad"):
        generator_io.rustfmt_source("fn", label="codec tables")

from __future__ import annotations

import json

from tools.hooks import session_learning


def test_teeth_extracts_crux_and_frontier_from_transcript(tmp_path):
    transcript = tmp_path / "transcript.jsonl"
    transcript.write_text(
        json.dumps(
            {
                "message": "Root cause: duplicate authority.\nNext frontier: delete old lane."
            }
        )
        + "\n",
        encoding="utf-8",
    )
    cruxes, frontiers = session_learning.extract_learning(transcript)
    assert cruxes == ["Root cause: duplicate authority."]
    assert frontiers == ["Next frontier: delete old lane."]


def test_record_writes_durable_digest_into_memory_corpus(tmp_path, monkeypatch):
    memory_dir = tmp_path / "memory"
    memory_dir.mkdir()
    (memory_dir / "MEMORY.md").write_text("# Memory\n", encoding="utf-8")
    transcript = tmp_path / "transcript.jsonl"
    transcript.write_text(
        json.dumps({"text": "Crux: one authority.\nOpen frontier: consumer migration."})
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        session_learning.memory_graph,
        "discover_memory_dir",
        lambda **kwargs: memory_dir,
    )
    monkeypatch.setattr(
        session_learning._common,
        "git_window_messages",
        lambda root, base: ["landed guard"],
    )
    monkeypatch.setattr(session_learning._common, "git_head", lambda root: "abc")
    path = session_learning.record(
        {"session_id": "s1", "cwd": str(tmp_path), "transcript_path": str(transcript)},
        tmp_path,
    )
    assert path is not None and path.is_file()
    text = path.read_text(encoding="utf-8")
    assert (
        "landed guard" in text
        and "Crux: one authority." in text
        and "Open frontier" in text
    )


def test_record_internal_failure_is_fail_open_at_stop_hook(tmp_path, monkeypatch):
    from tools.hooks import stop_gates

    monkeypatch.setattr(
        stop_gates._common,
        "read_hook_input",
        lambda: {"cwd": str(tmp_path), "session_id": "s"},
    )
    monkeypatch.setattr(stop_gates._common, "repo_root", lambda cwd=None: tmp_path)
    monkeypatch.setattr(
        stop_gates.session_learning,
        "record",
        lambda *args: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    monkeypatch.setattr(stop_gates, "GATES", [])
    assert stop_gates.run() == 0


def test_session_learning_self_test_is_live():
    assert session_learning.self_test()

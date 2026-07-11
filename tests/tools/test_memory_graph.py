"""Teeth for APPARATUS A10 -- the graph-memory recall engine + its two wired
consumers (session_digest nearest-memories, findings_memo_lint suggestions).

M05: a PASS is a hypothesis until reproduced -- these exercise the real parsers
on a controlled fixture corpus and assert the recall queries return the CORRECT
nodes, that a dangling ``[[link]]`` is REPORTED (not crashed on), and that both
consumers render AND fail open.
"""

from __future__ import annotations

import io
import sys
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.memory_graph as mg  # noqa: E402


# --------------------------------------------------------------------------
# Fixture corpus
# --------------------------------------------------------------------------


def _write_corpus(root: Path) -> Path:
    """A small but representative memory corpus:

    * MEMORY.md hooks for M01/M02/M03 (resolved) and M99 (NO POINTERS row).
    * POINTERS.md maps M01->alpha, M02->beta, M03->gamma; M50 inline (no file).
    * alpha: links to [[beta]] and dangling [[nowhere-note]]; cites M02;
      supersedes [[gamma]].
    * beta: links back to [[alpha]]; carries a unique keyword.
    * gamma: an older note (the superseded one).
    """
    mem = root / "memory"
    mem.mkdir(parents=True, exist_ok=True)
    (mem / "MEMORY.md").write_text(
        "# Memory index\n\n"
        "- M01 — alpha directive, the north star. See alpha\n"
        "- M02 — beta directive about widgets M01 references it\n"
        "- M03 — gamma directive (older)\n"
        "- M99 — orphan hook with no POINTERS row (unresolved directive)\n",
        encoding="utf-8",
    )
    (mem / "POINTERS.md").write_text(
        "# Memory pointer index\n\n"
        "| id | topic | file | section |\n"
        "|----|-------|------|---------|\n"
        "| M01 | Alpha directive | [alpha.md](alpha.md) | Core |\n"
        "| M02 | Beta directive | [beta.md](beta.md) | Core |\n"
        "| M03 | Gamma directive | [gamma.md](gamma.md) | Core |\n"
        "| M50 | (inline note) | — | Core |\n",
        encoding="utf-8",
    )
    (mem / "alpha.md").write_text(
        "# Alpha directive\n\n"
        "Alpha is the north star. It relates to [[beta]] and also to a note\n"
        "not yet written [[nowhere-note]]. This refines M02.\n\n"
        "This approach supersedes the older [[gamma]] plan entirely.\n",
        encoding="utf-8",
    )
    (mem / "beta.md").write_text(
        "# Beta directive\n\n"
        "Beta concerns quixoticwidget throughput and links back to [[alpha]].\n",
        encoding="utf-8",
    )
    (mem / "gamma.md").write_text(
        "# Gamma directive\n\nThe older gamma plan, now superseded.\n",
        encoding="utf-8",
    )
    # A note whose keywords match the session_digest lane context (witness /
    # build / apparatus) so the nearest-memories consumer surfaces something.
    (mem / "witness-note.md").write_text(
        "# Witness note\n\n"
        "The witness closure toward WASM numpy parity: frontend lowering seal "
        "and build wall-clock are the apparatus frontier.\n",
        encoding="utf-8",
    )
    return mem


def _memory_only(mem: Path) -> mg.MemoryGraph:
    return mg.build_graph(
        memory_dir=mem,
        include_memory=True,
        include_ledgers=False,
        include_findings=False,
        include_gates=False,
    )


# --------------------------------------------------------------------------
# Parsing + node model
# --------------------------------------------------------------------------


def test_build_parses_fixture(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    # The three topic files are memory nodes keyed by slug, aliased by M##.
    for slug, mid in (("alpha", "M01"), ("beta", "M02"), ("gamma", "M03")):
        n = g.node(slug)
        assert n is not None and n.type == "memory"
        assert mid in n.aliases
    # M## resolution goes both ways.
    assert g.resolve("M01") == "alpha"
    assert g.resolve("alpha") == "alpha"
    assert g.resolve("[[beta]]") == "beta"
    assert g.resolve("beta.md") == "beta"
    # M1 vs M01 padding normalizes.
    assert g.resolve("M1") == "alpha"


def test_counts_cover_edge_types(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    c = g.counts()
    assert c["nodes_by_type"].get("memory", 0) >= 3
    # links (alpha<->beta), cites (alpha->M02), supersedes (alpha->gamma).
    assert c["edges_by_type"].get("links", 0) >= 2
    assert c["edges_by_type"].get("supersedes", 0) >= 1


# --------------------------------------------------------------------------
# Recall queries
# --------------------------------------------------------------------------


def test_neighbors_bidirectional(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    nbrs = g.neighbors("alpha")
    ids = {n.id for n, _e, _d in nbrs}
    # alpha -> beta (out link) AND beta -> alpha (in link) both surface beta.
    assert "beta" in ids
    # gamma is reachable via the supersedes edge.
    assert "gamma" in ids
    # Querying by the M## alias yields the same neighborhood.
    ids_by_mid = {n.id for n, _e, _d in g.neighbors("M01")}
    assert ids == ids_by_mid


def test_neighbors_edge_type_filter(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    only_super = g.neighbors("alpha", edge_types=["supersedes"])
    assert {n.id for n, _e, _d in only_super} == {"gamma"}


def test_supersedes_forward_and_backward(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    assert [n.id for n in g.supersedes("alpha")] == ["gamma"]
    assert [n.id for n in g.superseded_by("gamma")] == ["alpha"]
    # A node that supersedes nothing returns empty (not an error).
    assert g.supersedes("beta") == []


def test_nearest_ranks_relevant_memory(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    ranked = g.nearest("quixoticwidget throughput concerns", k=2)
    assert ranked, "expected at least one nearby memory"
    assert ranked[0][0].id == "beta"  # the unique keyword pulls beta to the top
    # Scores are descending.
    scores = [s for _n, s in ranked]
    assert scores == sorted(scores, reverse=True)


def test_nearest_accepts_a_file(tmp_path):
    mem = _write_corpus(tmp_path)
    g = _memory_only(mem)
    ranked = g.nearest(str(mem / "beta.md"), k=1)
    assert ranked and ranked[0][0].id in {"beta", "alpha"}


# --------------------------------------------------------------------------
# Dangling links + unresolved M## (REPORTED, never fatal)
# --------------------------------------------------------------------------


def test_dangling_link_is_reported_not_crashed(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))  # building did NOT raise
    dsts = {e.dst for e in g.dangling_links()}
    assert "nowhere-note" in dsts  # the un-written forward reference
    # The dangling edge is NOT counted among resolved edges.
    assert all(e.resolved for e in g.edges)


def test_unresolved_mrefs(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    unresolved = g.unresolved_mrefs()
    assert unresolved == ["M99"]  # in MEMORY.md, no POINTERS row
    # M50 is an (inline note) row in POINTERS -> counts as RESOLVED.
    assert "M50" not in unresolved
    assert "M01" not in unresolved


# --------------------------------------------------------------------------
# what-consumes / what-produces (findings layer)
# --------------------------------------------------------------------------


def _fake_finding(fid, consumers, producers):
    return SimpleNamespace(
        finding_id=fid,
        one_line_summary=f"summary for {fid}",
        claim=f"claim for {fid}",
        consumers=tuple(consumers),
        producers=tuple(producers),
    )


def test_what_consumes_via_pipeline(tmp_path, monkeypatch):
    mem = _write_corpus(tmp_path)
    finding = _fake_finding(
        "widget_throughput_v1",
        consumers=("memory/M02", "tools/check_perf_freshness.py"),
        producers=("commit abc1234", "tools/bench_evidence.py"),
    )
    monkeypatch.setattr(
        "tools.findings_registry.query_findings", lambda: [finding], raising=False
    )
    g = mg.build_graph(
        memory_dir=mem,
        include_memory=True,
        include_ledgers=False,
        include_findings=True,
        include_gates=False,
    )
    # memory/M02 resolves ONTO the real beta node (no synthetic duplicate).
    consumers = {n.id for n in g.what_consumes("widget_throughput_v1")}
    assert "beta" in consumers
    assert "tools/check_perf_freshness.py" in consumers
    producers = {n.id for n in g.what_produces("widget_throughput_v1")}
    assert "commit:abc1234" in producers
    assert "tools/bench_evidence.py" in producers
    # The commit producer is typed as a decision node.
    assert g.node("commit:abc1234").type == "decision"


def test_what_consumes_public_api():
    g = mg.MemoryGraph()
    g.add_node("f_x_v1", "finding", title="x")
    g.add_node("tools/reader.py", "tool")
    g.add_edge("tools/reader.py", "f_x_v1", "consumes", "test")
    g.finalize()
    assert [n.id for n in g.what_consumes("f_x_v1")] == ["tools/reader.py"]
    assert g.what_consumes("nonexistent") == []


# --------------------------------------------------------------------------
# Obsidian export
# --------------------------------------------------------------------------


def test_obsidian_export_emits_wikilinks(tmp_path):
    g = _memory_only(_write_corpus(tmp_path))
    files = g.obsidian_export()
    assert "alpha.md" in files
    body = files["alpha.md"]
    assert "[[beta]]" in body  # a synthesized link re-emitted as a wikilink
    assert "supersedes::" in body and "[[gamma]]" in body


# --------------------------------------------------------------------------
# Consumer 1: session_digest nearest-memories (renders + fails open)
# --------------------------------------------------------------------------


def test_session_digest_renders_nearest(tmp_path, monkeypatch):
    from tools.hooks import session_digest as sd

    mem = _write_corpus(tmp_path)
    monkeypatch.setenv("MOLT_MEMORY_DIR", str(mem))
    out = io.StringIO()
    sd._section_nearest_memories(out, tmp_path, str(tmp_path))
    text = out.getvalue()
    assert "NEAREST MEMORIES" in text
    # The witness-flavored note matches the lane context and IS surfaced.
    assert "witness-note" in text


def test_session_digest_subdirectory_is_searchable_memory(tmp_path):
    mem = _write_corpus(tmp_path)
    session_dir = mem / "session_digests"
    session_dir.mkdir()
    (session_dir / "20260711-s1.md").write_text(
        "# Session learning s1\n\n## Crux learnings\n- forbidden checkout path policy\n",
        encoding="utf-8",
    )
    ranked = mg.nearest_memories(
        "forbidden checkout path policy",
        k=3,
        memory_dir=mem,
        repo_root=tmp_path,
    )
    assert any(node.id.startswith("memory:session_digests/") for node, _ in ranked)


def test_session_digest_nearest_fails_open(tmp_path, monkeypatch):
    from tools.hooks import session_digest as sd

    def _boom(*a, **k):
        raise RuntimeError("graph exploded")

    monkeypatch.setattr(mg, "nearest_memories", _boom, raising=True)
    out = io.StringIO()
    # Must NOT raise even though the graph raises inside.
    sd._section_nearest_memories(out, tmp_path, str(tmp_path))
    text = out.getvalue()
    assert "NEAREST MEMORIES" in text
    assert "recall unavailable" in text  # graceful fallback line


def test_session_digest_full_run_survives_graph_error(tmp_path, monkeypatch):
    from tools.hooks import session_digest as sd

    monkeypatch.setattr(
        mg,
        "nearest_memories",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")),
        raising=True,
    )
    monkeypatch.setattr(sd._common, "read_hook_input", lambda: {"cwd": str(tmp_path)})
    captured = io.StringIO()
    monkeypatch.setattr(sys, "stdout", captured)
    rc = sd.run()
    assert rc == 0
    assert "molt session digest" in captured.getvalue()


# --------------------------------------------------------------------------
# Consumer 2: findings_memo_lint suggestions (advisory, appear in output)
# --------------------------------------------------------------------------


def test_memo_lint_suggestion_appears(monkeypatch):
    import tools.findings_memo_lint as lint

    # A hand-built graph with one finding + one memory, so the suggestion is
    # deterministic (no dependence on the live registry).
    g = mg.MemoryGraph()
    g.add_node(
        "quixotic_throughput_v1",
        "finding",
        title="quixotic widget throughput speedup",
        keywords=["quixotic", "widget", "throughput", "speedup"],
    )
    g.add_node("tools/x.py", "tool")
    g.add_edge("tools/x.py", "quixotic_throughput_v1", "produces", "t")
    g.add_node(
        "widget-notes",
        "memory",
        title="widget notes",
        keywords=["quixotic", "widget", "throughput"],
    )
    g.finalize()
    monkeypatch.setattr(mg, "build_graph", lambda *a, **k: g, raising=True)

    v = lint.MemoViolation(
        "memory/demo.md", 3, "quixotic widget throughput is now 2.3x faster (measured)"
    )
    hints = lint.suggest_links([v])
    joined = " ".join(hints.get((v.file, v.line), []))
    assert "quixotic_throughput_v1" in joined
    assert "finding_id" in joined


def test_memo_lint_suggestion_fails_open(monkeypatch):
    import tools.findings_memo_lint as lint

    monkeypatch.setattr(
        mg,
        "build_graph",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("no graph")),
        raising=True,
    )
    v = lint.MemoViolation("memory/demo.md", 3, "2.3x faster measured")
    # No crash; simply no hints.
    assert lint.suggest_links([v]) == {}


# --------------------------------------------------------------------------
# check_memory_graph integrity tool
# --------------------------------------------------------------------------


def test_check_memory_graph_reports_counts(tmp_path):
    import tools.check_memory_graph as cmg

    mem = _write_corpus(tmp_path)
    report = cmg.analyze(memory_dir=mem)
    assert report["corpus_found"] is True
    assert report["unresolved_mref_count"] == 1  # M99
    assert report["dangling_count"] >= 1  # nowhere-note


def test_check_memory_graph_strict_exit_code(tmp_path):
    import tools.check_memory_graph as cmg

    mem = _write_corpus(tmp_path)
    # warn/--check never fails even with a dangling link + unresolved M##.
    assert cmg.main(["--check", "--memory-dir", str(mem)]) == 0
    # --strict fails on the unresolved M## (index break), not on danglers.
    assert cmg.main(["--strict", "--memory-dir", str(mem)]) == 1

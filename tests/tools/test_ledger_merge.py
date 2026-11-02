"""The ledger merge driver: findings rows merge by their ID, not by line.

Each case is a three-way merge whose right answer follows from what each side
did to which row. The last tests run the driver inside real Git rebases and
cherry-picks, the operations the landing queue performs, on the exact shapes
that conflicted on 2026-10-21: two lanes each adding a row at the top of a
section, and a lane editing a row next to one that main moved.
"""

from __future__ import annotations

import sys
import shutil
import time
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import ledger_merge

ROOT = Path(__file__).resolve().parents[2]


def _ledger(
    open_rows: list[str], fixed_rows: list[str], prose: str = "Open findings."
) -> str:
    def rows(items: list[str]) -> str:
        return "".join(f"| {item} | x |\n" for item in items)

    return (
        "# Ledger\n\n"
        "## Open\n\n"
        f"{prose}\n\n"
        "| ID | Finding |\n|----|---------|\n"
        f"{rows(open_rows)}\n"
        "## Fixed\n\n"
        "| ID | Fix |\n|----|-----|\n"
        f"{rows(fixed_rows)}"
    )


def _row(text: str, key: str, body: str) -> str:
    return text.replace(f"| {key} | x |", f"| {key} | {body} |")


def _merge(base: str, ours: str, theirs: str) -> ledger_merge.MergeResult:
    return ledger_merge.merge_documents(base, ours, theirs)


def _keys(text: str) -> list[str]:
    return [
        line.split("|")[1].strip()
        for line in text.splitlines()
        if ledger_merge._ROW.match(line)
    ]


def test_rows_both_sides_add_at_the_same_spot_are_both_kept() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _ledger(["HF-10", "HF-1", "HF-2"], ["HF-0"])
    theirs = _ledger(["HF-11", "HF-1", "HF-2"], ["HF-9", "HF-0"])

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _keys(result.text) == ["HF-11", "HF-10", "HF-1", "HF-2", "HF-9", "HF-0"]


def test_a_moved_and_rewritten_row_follows_its_side() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    # Main fixed HF-2: it leaves Open for the top of Fixed with new text.
    ours = _row(_ledger(["HF-1"], ["HF-2", "HF-0"]), "HF-2", "fixed by main")
    # The lane rewrote HF-1, the row next to it.
    theirs = _row(base, "HF-1", "lane progress")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _keys(result.text) == ["HF-1", "HF-2", "HF-0"]
    assert "| HF-1 | lane progress |" in result.text
    assert "| HF-2 | fixed by main |" in result.text
    assert result.text.count("| HF-2 |") == 1


def test_the_other_side_moves_a_row_the_frame_left_alone() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _row(base, "HF-1", "ours")
    theirs = _ledger(["HF-1"], ["HF-2", "HF-0"])

    result = _merge(base, ours, theirs)

    assert _keys(result.text) == ["HF-1", "HF-2", "HF-0"]
    assert "## Fixed" in result.text.split("| HF-2 |")[0]


def test_a_deleted_row_stays_deleted_and_an_edit_elsewhere_survives() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"])
    theirs = _row(base, "HF-0", "theirs")

    result = _merge(base, ours, theirs)

    assert _keys(result.text) == ["HF-1", "HF-0"]
    assert "| HF-0 | theirs |" in result.text


def test_one_side_changes_prose_and_the_other_changes_rows() -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"], prose="Open findings, all of them.")
    theirs = _row(_ledger(["HF-7", "HF-1"], ["HF-0"]), "HF-1", "theirs")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert "Open findings, all of them." in result.text
    assert _keys(result.text) == ["HF-7", "HF-1", "HF-0"]
    assert "| HF-1 | theirs |" in result.text


def test_both_sides_rewriting_one_row_is_a_conflict() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _row(base, "HF-1", "ours")
    theirs = _row(_row(base, "HF-1", "theirs"), "HF-2", "theirs too")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ("HF-1",)
    block = result.text.split("<<<<<<< ours\n")[1].split(">>>>>>> theirs\n")[0]
    assert block == "| HF-1 | ours |\n=======\n| HF-1 | theirs |\n"
    assert "| HF-2 | theirs too |" in result.text


@pytest.mark.parametrize(
    "ours_prose", ["Ours changed the prose.", "Ours changed it differently."]
)
def test_prose_both_sides_changed_falls_back_to_a_text_merge(ours_prose: str) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"], prose=ours_prose)
    theirs = _ledger(["HF-1"], ["HF-0"], prose="Theirs changed the prose.")

    with pytest.raises(ledger_merge._Unmergeable):
        _merge(base, ours, theirs)


def test_a_duplicated_row_key_falls_back_to_a_text_merge() -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    with pytest.raises(ledger_merge._Unmergeable):
        _merge(base, _ledger(["HF-1", "HF-1"], ["HF-0"]), base)


def _git(repo: Path, *args: str):
    return run_guarded_test_process(
        ["git", *args], cwd=str(repo), check=True, capture_output=True, text=True
    )


def _repo_with_driver(tmp_path: Path, base: str) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.email", "test@example.invalid")
    _git(repo, "config", "user.name", "Test")
    driver = f'"{sys.executable}" "{ROOT / "tools" / "ledger_merge.py"}" %O %A %B'
    _git(repo, "config", "merge.molt-ledger.driver", driver)
    (repo / ".gitattributes").write_text(
        "LEDGER.md merge=molt-ledger\n", encoding="utf-8"
    )
    (repo / "LEDGER.md").write_text(base, encoding="utf-8")
    _git(repo, "add", ".")
    _git(repo, "commit", "-q", "-m", "base")
    return repo


def test_a_cherry_pick_merges_two_rows_added_at_the_same_spot(tmp_path: Path) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(
        _ledger(["HF-11", "HF-1"], ["HF-0"]), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "lane opens HF-11")
    _git(repo, "switch", "-q", "main")
    (repo / "LEDGER.md").write_text(
        _ledger(["HF-10", "HF-1"], ["HF-0"]), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "main opens HF-10")

    _git(repo, "cherry-pick", "lane")

    assert _keys((repo / "LEDGER.md").read_text(encoding="utf-8")) == [
        "HF-11",
        "HF-10",
        "HF-1",
        "HF-0",
    ]


def test_a_rebase_merges_a_row_move_beside_a_row_edit(tmp_path: Path) -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(
        _row(base, "HF-1", "lane progress"), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "lane records HF-1")
    _git(repo, "switch", "-q", "main")
    fixed = _row(_ledger(["HF-1"], ["HF-2", "HF-0"]), "HF-2", "fixed by main")
    (repo / "LEDGER.md").write_text(fixed, encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "main fixes HF-2")
    _git(repo, "switch", "-q", "lane")

    _git(repo, "rebase", "main")

    text = (repo / "LEDGER.md").read_text(encoding="utf-8")
    assert _keys(text) == ["HF-1", "HF-2", "HF-0"]
    assert "| HF-1 | lane progress |" in text and "| HF-2 | fixed by main |" in text


def test_a_conflicting_row_stops_the_cherry_pick(tmp_path: Path) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(_row(base, "HF-1", "lane"), encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "lane")
    _git(repo, "switch", "-q", "main")
    (repo / "LEDGER.md").write_text(_row(base, "HF-1", "main"), encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "main")

    picked = run_guarded_test_process(
        ["git", "cherry-pick", "lane"], cwd=str(repo), capture_output=True, text=True
    )

    assert picked.returncode != 0
    assert "<<<<<<< ours" in (repo / "LEDGER.md").read_text(encoding="utf-8")


def _with_families(text: str, families: dict[str, tuple[str, str]]) -> str:
    rows = "".join(
        f"| {letter}. {name} | apparatus | - | cause | {keys} |\n"
        for letter, (name, keys) in families.items()
    )
    return text.replace(
        "## Fixed\n",
        "### Family dependency order\n\n"
        "| Family | Stratum | HF-459 axes | Cause | Keys |\n|---|---|---|---|---|\n"
        f"{rows}\n"
        "## Fixed\n",
    )


def _family_keys(text: str, letter: str) -> str:
    line = next(line for line in text.splitlines() if line.startswith(f"| {letter}. "))
    return line.strip().strip("|").split("|")[-1].strip()


def test_family_keys_merge_as_a_set_when_both_sides_edit_one_family() -> None:
    # The 2026-10-29 refusal: one lane opened a row in family C, main closed
    # another; both edited C's key list, so the old driver merged as text.
    base = _with_families(
        _ledger(["HF-1", "HF-2"], ["HF-0"]), {"C": ("Cells", "HF-1, HF-2")}
    )
    ours = _with_families(
        _ledger(["HF-10", "HF-1", "HF-2"], ["HF-0"]),
        {"C": ("Cells", "HF-1, HF-2, HF-10")},
    )
    theirs = _with_families(
        _ledger(["HF-1"], ["HF-2", "HF-0"]), {"C": ("Cells", "HF-1")}
    )

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _family_keys(result.text, "C") == "HF-1, HF-10"
    assert _keys(result.text) == ["HF-10", "HF-1", "HF-2", "HF-0"]


def test_both_sides_renaming_one_family_is_a_conflict() -> None:
    base = _with_families(_ledger(["HF-1"], []), {"C": ("Cells", "HF-1")})
    ours = _with_families(_ledger(["HF-1"], []), {"C": ("Graph cells", "HF-1")})
    theirs = _with_families(_ledger(["HF-1"], []), {"C": ("Proof cells", "HF-1")})

    result = _merge(base, ours, theirs)

    assert result.conflicts == ("family C",)


def test_generated_merge_attributes_are_the_manifest_projection() -> None:
    from tools import generated_merge

    assert generated_merge.generated_outputs()[ROOT / ".gitattributes"] == (
        ROOT / ".gitattributes"
    ).read_text(encoding="utf-8")
    owners = generated_merge.output_owners(ROOT)
    result = _git(ROOT, "check-attr", "merge", "--", *owners)
    attributes = {
        line.rsplit(": ", 2)[0]: line.rsplit(": ", 1)[1]
        for line in result.stdout.splitlines()
    }
    for path, owner in owners.items():
        assert (attributes[path] == "binary") == owner.eligible


def _generated_ledger(keys: list[str]) -> str:
    header = "| ID | Finding | Evidence |\n|---|---|---|\n"
    return (
        "# Ledger\n\n## Open\n\n"
        + header
        + "".join(f"| {key} | x | evidence |\n" for key in keys)
        + "\n## Fixed\n\n"
        + header
        + "| HF-0 | x | evidence |\n"
    )


@pytest.fixture
def generated_repo(tmp_path: Path, monkeypatch):
    """Real generator/CLI sources, selected image, and two independently authored rows."""
    from tools import gen_finding_ids, generated_merge

    repo = tmp_path / "repo"
    repo.mkdir()
    for directory in ("tools", "src", "config"):
        shutil.copytree(
            ROOT / directory,
            repo / directory,
            ignore=shutil.ignore_patterns("__pycache__", "node_modules"),
        )
    for file in (
        "pyproject.toml",
        "uv.lock",
        ".python-version",
        "Cargo.toml",
        ".gitignore",
    ):
        shutil.copyfile(ROOT / file, repo / file)
    # The selected producer needs no external toolchain. Keep the real runner,
    # import boundary and guard, with just this proof cell's declared producers.
    (repo / "tools/proof_plan.toml").write_text("", encoding="utf-8")
    manifest = """schema_version = 1
[[generator]]
tool = "tools/gen_finding_ids.py"
outputs = ["tests/findings/finding_ids.py"]
check_mode = true
idempotent = true
toolchains = []
[[generator]]
tool = "tools/generated_merge.py"
outputs = [".gitattributes"]
check_mode = true
idempotent = true
toolchains = []
[[generator]]
tool = "tools/external.py"
outputs = ["external.txt"]
ci_checkable = false
ci_skip_reason = "external oracle is absent"
idempotent = true
check_mode = true
toolchains = []
"""
    (repo / "tools/generator_manifest.toml").write_text(manifest, encoding="utf-8")
    with monkeypatch.context() as patch:
        patch.setattr(generated_merge, "ROOT", repo)
        (repo / ".gitattributes").write_text(
            generated_merge.generated_outputs()[repo / ".gitattributes"],
            encoding="utf-8",
        )
    ledger = repo / "docs/agent/V1_HANDOFF_FINDINGS.md"
    ledger.parent.mkdir(parents=True)
    ledger.write_text(_generated_ledger(["HF-1"]), encoding="utf-8")
    output = repo / "tests/findings/finding_ids.py"
    output.parent.mkdir(parents=True)
    output.write_text(gen_finding_ids.render(repo), encoding="utf-8")
    (repo / "external.txt").write_text("base\n", encoding="utf-8")
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.name", "Test")
    _git(repo, "config", "user.email", "test@example.invalid")
    _git(
        repo,
        "config",
        "merge.molt-ledger.driver",
        f'"{sys.executable}" -B "{repo / "tools/ledger_merge.py"}" %O %A %B',
    )
    # Support sources stay outside the fixture's branch content. Only the
    # actual merge inputs are versioned; neither source is a fake generator.
    (repo / ".git/info/exclude").write_text(
        "tools/\nsrc/\nconfig/\npyproject.toml\nuv.lock\nCargo.toml\n", encoding="utf-8"
    )
    _git(
        repo,
        "add",
        ".gitattributes",
        ".gitignore",
        ".python-version",
        "docs",
        "tests",
        "external.txt",
    )
    _git(repo, "commit", "-qm", "base")
    base = _git(repo, "rev-parse", "HEAD").stdout.strip()
    _git(repo, "branch", "lane")
    for branch, key in (("main", "HF-10"), ("lane", "HF-11")):
        _git(repo, "switch", "-q", branch)
        ledger.write_text(_generated_ledger([key, "HF-1"]), encoding="utf-8")
        output.write_text(gen_finding_ids.render(repo), encoding="utf-8")
        _git(repo, "commit", "-qam", f"open {key}")
    _git(repo, "switch", "-q", "main")
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", sys.prefix)
    return repo, base, ledger, output


def _pending_merge(repo: Path, ref: str = "lane"):
    return run_guarded_test_process(
        ["git", "merge", "--no-edit", ref], cwd=repo, capture_output=True, text=True
    )


def _assert_generated_union(repo, ledger, output):
    from tools import gen_finding_ids

    # Independent membership oracle plus byte identity with the canonical producer.
    assert set(_keys(ledger.read_text(encoding="utf-8"))) == {
        "HF-0",
        "HF-1",
        "HF-10",
        "HF-11",
    }
    assert output.read_text(encoding="utf-8") == gen_finding_ids.render(repo)
    assert _git(
        repo, "show", "HEAD:tests/findings/finding_ids.py"
    ).stdout == output.read_text(encoding="utf-8")
    assert _git(repo, "status", "--porcelain").stdout == ""
    assert (
        len(_git(repo, "rev-list", "--parents", "-n", "1", "HEAD").stdout.split()) == 3
    )


def _landing_generated_candidate(repo: Path, ref: str = "lane"):
    from tools import molt_dev_queue as queue

    upstream = _git(repo, "rev-parse", "main").stdout.strip()
    tip = _git(repo, "rev-parse", ref).stdout.strip()
    request = queue.LandingRequest(
        "merge-fixture", tip, str(repo), ref, "2026-11-01T08:00:00+00:00"
    )
    calls = []

    def run(args):
        calls.append(tuple(args))
        result = run_guarded_test_process(
            ["git", *args], cwd=repo, capture_output=True, text=True
        )
        return queue.GitResult(result.returncode, result.stdout, result.stderr)

    return queue.build_candidate(run, upstream, (request,)), calls


@pytest.mark.parametrize("entry", ["resolver", "worker", "runner"])
def test_generated_ledger_merge_uses_the_merged_source_tree(
    generated_repo, entry, capsys, record_property
):
    from tools import generated_merge

    repo, _base, ledger, output = generated_repo
    started = time.perf_counter()
    if entry == "worker":
        from molt.process_environment import ProcessEnvironment
        import os

        environment = ProcessEnvironment(os.environ).updated(
            {"PYTHONPATH": os.pathsep.join((str(repo / "src"), str(repo)))}
        )
        result = run_guarded_test_process(
            [
                sys.executable,
                "-B",
                str(repo / "tools/generated_merge.py"),
                "merge",
                "lane",
            ],
            cwd=repo,
            env=environment,
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        rendered = result.stdout
    elif entry == "resolver":
        merge = _pending_merge(repo)
        assert merge.returncode != 0
        assert (
            _git(repo, "diff", "--name-only", "--diff-filter=U").stdout.strip()
            == "tests/findings/finding_ids.py"
        )
        assert "<<<<<<<" not in output.read_text(encoding="utf-8")
        assert generated_merge.resolve_generated_merge(lambda args: _git(repo, *args))
        rendered = capsys.readouterr().out
    else:
        result, _calls = _landing_generated_candidate(repo)
        assert len(result.included) == 1 and result.conflicted == ()
        rendered = capsys.readouterr().out
    assert rendered.count("s tools/gen_finding_ids.py") == 1
    _assert_generated_union(repo, ledger, output)
    elapsed = time.perf_counter() - started
    record_property("merge_elapsed_seconds", elapsed)
    record_property("producer_renders", 1)
    print(f"generated merge {entry}: {elapsed:.6f}s; one producer render")


def test_generated_merge_preserves_source_conflicts_without_running_a_generator(
    generated_repo,
):
    from tools import generated_merge

    repo, _base, ledger, output = generated_repo
    ledger.write_text(
        _row(ledger.read_text(encoding="utf-8"), "HF-1", "main edit"), encoding="utf-8"
    )
    _git(repo, "commit", "-qam", "main edits shared row")
    _git(repo, "switch", "-q", "lane")
    ledger.write_text(
        _row(ledger.read_text(encoding="utf-8"), "HF-1", "lane edit"), encoding="utf-8"
    )
    _git(repo, "commit", "-qam", "lane edits shared row")
    _git(repo, "switch", "-q", "main")
    assert _pending_merge(repo).returncode != 0
    before = {p: p.read_bytes() for p in (ledger, output)}
    index = _git(repo, "ls-files", "--stage").stdout
    assert (
        generated_merge.resolve_generated_merge(lambda args: _git(repo, *args)) is False
    )
    assert {p: p.read_bytes() for p in before} == before
    assert _git(repo, "ls-files", "--stage").stdout == index


def test_generated_merge_generator_error_does_not_commit_ours(generated_repo):
    from tools import generated_merge

    repo, _base, ledger, output = generated_repo
    assert _pending_merge(repo).returncode != 0
    # A real malformed merged ledger is rejected by the real producer.
    text = ledger.read_text(encoding="utf-8")
    ledger.write_text(text + "\n" + "| HF-1 | duplicate | x |\n", encoding="utf-8")
    _git(repo, "add", "docs/agent/V1_HANDOFF_FINDINGS.md")
    before = _git(repo, "rev-parse", "HEAD").stdout
    with pytest.raises(
        generated_merge.GeneratedMergeError, match="structural problems"
    ):
        generated_merge.resolve_generated_merge(lambda args: _git(repo, *args))
    assert _git(repo, "rev-parse", "HEAD").stdout == before
    assert (
        _git(repo, "diff", "--name-only", "--diff-filter=U").stdout.strip()
        == "tests/findings/finding_ids.py"
    )


def test_external_generator_conflict_names_its_owner(generated_repo):
    from tools import generated_merge

    repo, _base, _ledger_path, _output = generated_repo
    for branch in ("main", "lane"):
        _git(repo, "switch", "-q", branch)
        (repo / "external.txt").write_text(branch + "\n", encoding="utf-8")
        _git(repo, "commit", "-qam", branch)
    _git(repo, "switch", "-q", "main")
    assert _pending_merge(repo).returncode != 0
    with pytest.raises(
        generated_merge.GeneratedMergeError,
        match="tools/external.py: external oracle is absent",
    ):
        generated_merge.resolve_generated_merge(lambda args: _git(repo, *args))
    assert "<<<<<<<" in (repo / "external.txt").read_text(encoding="utf-8")


def test_clean_generated_merge_starts_no_resolver_or_producer(
    generated_repo, monkeypatch, record_property
):
    from tools import generated_merge

    repo, base, ledger, output = generated_repo
    expected = output.read_bytes()
    _git(repo, "switch", "-qc", "unrelated", base)
    (repo / "source.txt").write_text("independent source\n", encoding="utf-8")
    _git(repo, "add", "source.txt")
    _git(repo, "commit", "-qm", "source only")
    _git(repo, "switch", "-q", "main")

    def forbidden(_run):
        pytest.fail("clean merge invoked the generated resolver")

    monkeypatch.setattr(generated_merge, "resolve_generated_merge", forbidden)
    started = time.perf_counter()
    candidate, calls = _landing_generated_candidate(repo, "unrelated")
    elapsed = time.perf_counter() - started
    assert len(candidate.included) == 1 and candidate.conflicted == ()
    assert output.read_bytes() == expected
    assert not any(args[0] in ("diff", "add", "commit") for args in calls)
    assert _git(repo, "status", "--porcelain").stdout == ""
    record_property("merge_elapsed_seconds", elapsed)
    record_property("producer_renders", 0)
    record_property("resolver_calls", 0)
    print(
        f"clean landing merge: {elapsed:.6f}s; zero resolver/producer calls; {len(calls)} existing Git calls"
    )


@pytest.mark.parametrize("boundary", ["pin", "image"])
def test_merge_regeneration_refuses_another_interpreter_identity(
    tmp_path, monkeypatch, boundary
):
    from tools import generators, venv_exec

    monkeypatch.setattr(generators, "ROOT", tmp_path)
    pin = ".".join(map(str, sys.version_info[:3]))
    (tmp_path / ".python-version").write_text(
        "3.12.0" if boundary == "pin" else pin, encoding="utf-8"
    )
    monkeypatch.setattr(
        venv_exec,
        "resolve_venv",
        lambda **kwargs: (
            Path(sys.prefix) if boundary == "pin" else tmp_path / "foreign-image"
        ),
    )
    with pytest.raises(ValueError, match="requires the selected CPython"):
        generators.require_merge_python()


def test_landing_generated_failure_keeps_its_error_and_restores_the_candidate(
    generated_repo,
):
    repo, _base, _ledger_path, _output = generated_repo
    before = _git(repo, "rev-parse", "HEAD").stdout.strip()
    producer = repo / "tools/gen_finding_ids.py"
    source = producer.read_text(encoding="utf-8")
    assert "return {_REPO_ROOT / OUTPUT: render()}" in source
    producer.write_text(
        source.replace(
            "return {_REPO_ROOT / OUTPUT: render()}",
            'raise ValueError("injected producer failure after source merge")',
        ),
        encoding="utf-8",
    )
    candidate, _calls = _landing_generated_candidate(repo)
    assert candidate.included == ()
    assert len(candidate.conflicted) == 1
    assert "injected producer failure after source merge" in candidate.conflicted[0][1]
    assert _git(repo, "rev-parse", "HEAD").stdout.strip() == before
    assert _git(repo, "status", "--porcelain").stdout == ""


def test_merge_regeneration_names_an_unavailable_selected_image(tmp_path, monkeypatch):
    from tools import generated_merge

    monkeypatch.delenv("MOLT_VENV", raising=False)
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(tmp_path / "missing-image"))
    with pytest.raises(
        generated_merge.GeneratedMergeError, match="virtualenv Python not found"
    ):
        generated_merge.regenerate(ROOT, ("tools/gen_finding_ids.py",))


def test_membership_edges_and_findings_have_distinct_merge_identity() -> None:
    from tests.integration_driver_fixture import FIXTURE_LEDGER
    from tools import finding_status

    # One lane changes classification and an edge; the other fixes an
    # unrelated row's prose. The same HF key occurs in all three tables.
    base = FIXTURE_LEDGER
    ours = base.replace("| HF-3 | post_1_0 |", "| HF-3 | apparatus |")
    ours = ours.replace(
        "| HF-2 | HF-7 | seven consumes two |",
        "| HF-9 | HF-7 | seven consumes closed nine |",
    )
    theirs = base.replace("| HF-1 | d | e | n |", "| HF-1 | actual defect | e | n |")
    result = _merge(base, ours, theirs)
    assert result.conflicts == ()
    status = finding_status.project(result.text)
    assert status.problems == ()
    assert status.classes["HF-3"] == finding_status.V1Class.APPARATUS
    assert any(
        e.source == "HF-9" and e.target == "HF-7" for e in status.dependency_edges
    )
    assert "| HF-1 | actual defect | e | n |" in result.text


def test_first_membership_and_edge_insertions_stay_in_their_own_tables() -> None:
    from tests.integration_driver_fixture import FIXTURE_LEDGER
    from tools import finding_status

    # Both sides add at the first row of a metadata table. There is no
    # preceding metadata row to anchor either addition.
    base = FIXTURE_LEDGER
    ours = base.replace("| HF-1 | d | e | n |", "| HF-1 | changed defect | e | n |")
    theirs = (
        base.replace(
            "| HF-1 | release_blocking |",
            "| HF-8 | apparatus |\n| HF-1 | release_blocking |",
        )
        .replace(
            "| A. First | engine | loop/per_op | a | HF-1, HF-3, HF-4 |",
            "| A. First | engine | loop/per_op | a | HF-1, HF-3, HF-4, HF-8 |",
        )
        .replace(
            "| HF-1 | d | e | n |",
            "| HF-8 | independent apparatus | e | n |\n| HF-1 | d | e | n |",
        )
    )
    theirs = theirs.replace(
        "| A | B | b needs a |",
        "| HF-9 | HF-6 | closed prerequisite |\n| A | B | b needs a |",
    )
    result = _merge(base, ours, theirs)
    assert result.conflicts == ()
    status = finding_status.project(result.text)
    assert len(status.v1_membership) == 8
    assert len(status.dependency_edges) == 4
    assert finding_status.dependency_problems(status) == []
    assert "| HF-1 | changed defect | e | n |" in result.text

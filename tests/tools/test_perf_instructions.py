"""The instruction signal on main: counters, registry, comparison and store.

The comparison tests use arithmetic oracles: each threshold term is probed just
above and just below its own boundary. The store tests use a real Git history.
"""

from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import sys
from collections.abc import Iterable, Sequence
from typing import Any

import pytest

from tests.process_guard_common import run_guarded_test_process

from tools import perf_instructions as pi
from tools.bench_suites import BENCHMARKS
from tools.perf_primitives import per_op_axes

REPO_ROOT = Path(__file__).resolve().parents[2]
DIGEST = "d" * 64


# --- counters ----------------------------------------------------------------


@pytest.mark.parametrize(
    "counter,text,expected",
    [
        (
            pi.Counter.MACOS,
            "   123456 instructions retired\n 43 cycles elapsed\n",
            123456,
        ),
        (pi.Counter.PERF, "98765;;instructions:u;123;100.00;;\n", 98765),
        (pi.Counter.CALLGRIND, "events: Dr Ir Dw\nsummary: 12 345 67\n", 345),
    ],
)
def test_counter_protocols(counter: pi.Counter, text: str, expected: int) -> None:
    assert pi.parse_count(counter, text) == expected


@pytest.mark.parametrize(
    "text",
    [
        "",
        "0 instructions retired",
        "3 instructions retired\n4 instructions retired",
        "nan instructions retired",
    ],
)
def test_missing_or_ambiguous_counts_refuse(text: str) -> None:
    with pytest.raises(ValueError):
        pi.parse_count(pi.Counter.MACOS, text)


@pytest.mark.parametrize("percent", ["99.99", "nan", "inf", "100.01"])
def test_perf_multiplexing_refuses(percent: str) -> None:
    with pytest.raises(ValueError, match="multiplexed"):
        pi.parse_count(pi.Counter.PERF, f"100;;instructions:u;300;{percent};;\n")


def _host_counter() -> pi.Counter:
    try:
        return pi.select_counter("auto")
    except ValueError as exc:
        pytest.skip(f"no instruction counter on this host: {exc}")


@pytest.mark.parametrize(
    "code,timeout,timed_out",
    [
        ("raise SystemExit(3)", 30.0, False),
        ("import os, signal; os.kill(os.getpid(), signal.SIGKILL)", 30.0, False),
        ("import time; time.sleep(30)", 1.0, True),
    ],
)
def test_the_measured_program_failing_is_an_outcome_not_an_error(
    tmp_path: Path, code: str, timeout: float, timed_out: bool
) -> None:
    counter = _host_counter()
    outcome = pi.observe(
        counter,
        [sys.executable, "-I", "-c", code],
        tmp_path / "program",
        dict(os.environ),
        timeout,
    )
    assert isinstance(outcome, pi.ProgramFailed)
    assert outcome.timed_out is timed_out


def test_a_measured_program_has_a_positive_count(tmp_path: Path) -> None:
    counter = _host_counter()
    outcome = pi.observe(
        counter,
        [sys.executable, "-I", "-c", "print('x')"],
        tmp_path / "program",
        dict(os.environ),
        30.0,
    )
    assert isinstance(outcome, pi.Measured)
    assert outcome.observation.instructions > 0
    assert outcome.observation.stdout == "x\n"


# --- registry ----------------------------------------------------------------


def test_registry_partitions_every_program_into_one_tier() -> None:
    hourly, nightly = pi.workloads(pi.Tier.HOURLY), pi.workloads(pi.Tier.NIGHTLY)
    assert (len(hourly), len(nightly)) == (74, 5)
    assert pi.NIGHTLY_BENCHMARKS <= set(BENCHMARKS)
    assert {w.bench for w in nightly} == pi.NIGHTLY_BENCHMARKS
    names = [w.name for w in (*hourly, *nightly)]
    assert len(names) == len(set(names)) == 1 + len(pi.PRIMITIVES) + len(BENCHMARKS)
    assert per_op_axes() == tuple(f"{p.name}/per_op" for p in pi.PRIMITIVES)
    empty = next(w for w in hourly if w.name == "empty")
    assert (empty.phases(), empty.arguments()) == ((pi.Phase.STARTUP,), (0,))


def test_the_instrument_is_exactly_the_imported_measurement_path() -> None:
    """The run identity hashes INSTRUMENT; a new import must join it."""
    source = (REPO_ROOT / "tools/perf_instructions.py").read_text(encoding="utf-8")
    imported = set()
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.ImportFrom) and (node.module or "").startswith("tools"):
            if node.module == "tools":
                imported.update(f"tools/{alias.name}.py" for alias in node.names)
            else:
                imported.add(node.module.replace(".", "/") + ".py")
    for path in imported:
        assert (REPO_ROOT / path).is_file(), path
    # The suspect commands run after the record is written; they cannot move
    # a count, and that module's churn would reset every baseline.
    assert imported - {"tools/perf_instructions.py"} == (
        set(pi.INSTRUMENT) - {"tools/perf_instructions.py"}
    ) | {"tools/molt_dev_queue.py"}


def test_declared_primitive_programs_have_independent_cpython_oracles(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    expected = {
        "loop": "4950",
        "id": "True",
        "memo_get": "100",
        "memo_set": "64",
        "type_dispatch": "100",
        "getattr_miss": "100",
        "dynamic_call": "4950",
        "isinstance": "0",
        "tuple_unpack": "9900",
        "reduce_ex": "50",
        "new_and_update": "10",
        "recursion": "100",
        "plain_construction": "(99, 2)",
        "dataclass_construction": "(99, 2)",
        "raise_user": "100",
        "raise_runtime": "100",
    }
    assert {p.name for p in pi.PRIMITIVES} == set(expected)
    monkeypatch.setattr("sys.argv", ["primitive.py", "100"])
    for primitive in pi.PRIMITIVES:
        exec(
            compile(primitive.program(), "primitive.py", "exec"),
            {"__name__": "__main__"},
        )
        assert capsys.readouterr().out == expected[primitive.name] + "\n"


def test_id_witness_rejects_a_nonunique_identity_provider(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    primitive = next(p for p in pi.PRIMITIVES if p.name == "id")
    monkeypatch.setattr("sys.argv", ["primitive.py", "100"])
    # Wire-level negative: 64 distinct live objects cannot all have identity 1.
    exec(
        compile(primitive.program(), "primitive.py", "exec"),
        {"__name__": "__main__", "id": lambda obj: 1},
    )
    assert capsys.readouterr().out == "False\n"


def test_primitive_program_is_its_own_project_inside_a_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli.project_roots import _find_project_root

    checkout = tmp_path / "checkout"
    out = checkout / "proof-receipts" / "evidence" / "instructions" / "programs"
    out.mkdir(parents=True)
    (checkout / "pyproject.toml").write_text(
        "[project]\nname = 'x'\n", encoding="utf-8"
    )
    program = out / "loop.py"
    program.write_text("print(1)\n", encoding="utf-8")
    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    # Unpinned, the build would adopt the enclosing checkout and its sources.
    assert _find_project_root(program) == checkout.resolve()
    ambient = {"MOLT_PROJECT_ROOT": str(checkout), "PATH": "/bin"}
    env = pi.primitive_build_env(ambient, {"MOLT_BUILD": "x"}, out)
    assert env["PATH"] == "/bin" and env["MOLT_BUILD"] == "x"
    assert (env["PYTHONHASHSEED"], env["MOLT_KEEP_SYMBOLS"]) == ("0", "1")
    monkeypatch.setenv("MOLT_PROJECT_ROOT", env["MOLT_PROJECT_ROOT"])
    assert _find_project_root(program) == out.resolve()


# --- comparison ----------------------------------------------------------------


def _workload(run_args: Sequence[str] = ()) -> dict[str, Any]:
    return {
        "schema": "molt.benchmark-workload/2",
        "entry": "tests/benchmarks/bench_x.py",
        "entry_source": "tests/benchmarks/bench_x.py",
        "scope": "local-python-source-closure",
        "sources": [
            {"path": "tests/benchmarks/bench_x.py", "size": 1, "sha256": "a" * 64}
        ],
        "topology_sha256": "b" * 64,
        "run_args": list(run_args),
        "molt_args": [],
        "sealed_inputs": None,
    }


def _axis(
    name: str,
    samples: Iterable[float] = (),
    *,
    unmeasured: str | None = None,
    run_args: Sequence[str] = (),
) -> dict[str, Any]:
    row = {"axis": name, "workload": _workload(run_args)}
    if unmeasured is None:
        row["samples"] = list(samples)
    else:
        row.update(unmeasured=unmeasured, detail="")
    return row


def _record(
    commit: str,
    axes: Iterable[dict[str, Any]],
    *,
    recorded_at: str = "2026-11-10T00:00:00+00:00",
) -> dict[str, Any]:
    return {
        "schema": pi.RECORD_SCHEMA,
        "commit": commit,
        "identity": {"tier": "hourly"},
        "identity_digest": DIGEST,
        "recorded_at": recorded_at,
        "workloads": [],
        "axes": list(axes),
    }


def _verdicts(
    base_axes: Iterable[dict[str, Any]],
    head_axes: Iterable[dict[str, Any]],
    bounds: dict[str, float] | None = None,
) -> dict[str, list[str]]:
    result = pi.compare(
        _record("a" * 40, base_axes), _record("b" * 40, head_axes), bounds or {}
    )
    return {kind: [row["axis"] for row in rows] for kind, rows in result.items()}


@pytest.mark.parametrize(
    "head,bound,expected",
    [
        # The 1% floor alone.
        ((1011,) * 5, 0.0, "regressed"),
        ((1009,) * 5, 0.0, "same"),
        ((989,) * 5, 0.0, "improved"),
        # 3 x the calibrated bound (1%) gives 3%.
        ((1031,) * 5, 0.01, "regressed"),
        ((1029,) * 5, 0.01, "same"),
        # 2 x the head spread: median 1100, spread 1% gives 2%; +10% regresses.
        ((1100, 1100, 1100, 1100, 1111), 0.0, "regressed"),
        # Median 1030 with spread 30/1030 gives 5.8%: +3% stays the same.
        ((1000, 1030, 1030, 1030, 1060), 0.0, "same"),
    ],
)
def test_each_threshold_term_bounds_the_verdict(
    head: tuple[int, ...], bound: float, expected: str
) -> None:
    base = [_axis("x/whole", (1000,) * 5)]
    key = pi.axis_key(_axis("x/whole", head))
    verdicts = _verdicts(base, [_axis("x/whole", head)], {key: bound})
    assert verdicts[expected] == ["x/whole"]


@pytest.mark.parametrize("reason", list(pi.Unmeasured))
def test_an_axis_the_head_cannot_measure_is_lost(reason: pi.Unmeasured) -> None:
    verdicts = _verdicts(
        [_axis("x/whole", (1000,) * 5)], [_axis("x/whole", unmeasured=reason.value)]
    )
    assert verdicts["lost"] == ["x/whole"]


def test_incomparable_or_unmeasured_base_axes_are_new() -> None:
    base = [
        _axis("changed/whole", (1000,) * 5, run_args=("1",)),
        _axis("broken/whole", unmeasured="run_failed"),
    ]
    head = [
        _axis("changed/whole", (2000,) * 5, run_args=("2",)),
        _axis("broken/whole", (1000,) * 5),
        _axis("added/whole", (1000,) * 5),
    ]
    assert sorted(_verdicts(base, head)["new"]) == [
        "added/whole",
        "broken/whole",
        "changed/whole",
    ]


def test_records_of_different_identities_never_compare() -> None:
    other = {**_record("b" * 40, []), "identity_digest": "e" * 64}
    with pytest.raises(ValueError, match="different run identities"):
        pi.compare(_record("a" * 40, []), other, {})


def test_the_bound_comes_from_the_first_record_that_measured_the_axis(
    tmp_path: Path,
) -> None:
    store = pi.RecordStore(tmp_path)
    first = _axis("x/whole", (100, 100, 100, 100, 104))
    later = _axis("x/whole", (100,) * 5)
    store.put(
        _record(
            "1" * 40,
            [_axis("x/whole", unmeasured="run_failed")],
            recorded_at="2026-11-01T00:00:00+00:00",
        )
    )
    store.put(_record("2" * 40, [first], recorded_at="2026-11-02T00:00:00+00:00"))
    store.put(_record("3" * 40, [later], recorded_at="2026-11-03T00:00:00+00:00"))
    # Spread 4/100 from the median 100: 2 x 4% = 8%.
    assert store.first_bounds(DIGEST) == {pi.axis_key(first): 0.08}
    with pytest.raises(FileExistsError):
        store.put(_record("3" * 40, [later]))


# --- store and verdict over real Git history ---------------------------------


def _git(repo: Path, *argv: str, when: int | None = None) -> str:
    env = dict(os.environ)
    if when is not None:
        stamp = f"{when} +0000"
        env.update(GIT_AUTHOR_DATE=stamp, GIT_COMMITTER_DATE=stamp)
    return run_guarded_test_process(
        ["git", "-C", str(repo), *argv], env=env, timeout=60, check=True
    ).stdout.strip()


@pytest.fixture
def history(tmp_path: Path) -> tuple[Path, list[str]]:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.email", "t@example.com")
    _git(repo, "config", "user.name", "t")
    now = 1_800_000_000
    commits = []
    for index, days in enumerate((10, 8, 1, 0)):
        (repo / "f").write_text(str(index), encoding="utf-8")
        _git(repo, "add", "f")
        _git(repo, "commit", "-q", "-m", f"c{index}", when=now - days * 86400)
        commits.append(_git(repo, "rev-parse", "HEAD"))
    return repo, commits


def test_verdicts_against_the_nearest_measured_ancestor(
    tmp_path: Path, history: tuple[Path, list[str]]
) -> None:
    repo, (c0, c1, c2, c3) = history
    store = pi.RecordStore(tmp_path / "store")
    store.put(_record(c1, [_axis("x/whole", (1000,) * 5)]))
    assert (
        pi.verdict(store, repo, _record(c1, [_axis("x/whole", (1000,) * 5)]))["status"]
        == pi.PerfStatus.UNARMED
    )
    green = pi.verdict(store, repo, _record(c3, [_axis("x/whole", (1005,) * 5)]))
    assert (green["status"], green["base"], green["suspects"]) == ("green", c1, [])
    red = pi.verdict(store, repo, _record(c3, [_axis("x/whole", (1100,) * 5)]))
    assert (red["status"], red["base"]) == ("red", c1)
    assert [row["axis"] for row in red["regressed"]] == ["x/whole"]
    # Postsubmit's field names and its restoration commands, newest first.
    assert red["suspects"] == [c3, c2]
    assert red["revert_commands"] == [
        f"git revert --no-edit {c3}",
        f"git revert --no-edit {c2}",
    ]
    # A nearer measured ancestor becomes the base; the window bounds the walk.
    store.put(_record(c2, [_axis("x/whole", (1100,) * 5)]))
    assert (
        pi.verdict(store, repo, _record(c3, [_axis("x/whole", (1100,) * 5)]))["base"]
        == c2
    )
    assert store.nearest_measured_ancestor(repo, c3, DIGEST, window=0) is None


def test_creep_compares_with_the_record_nearest_a_week_before(
    tmp_path: Path, history: tuple[Path, list[str]]
) -> None:
    repo, (c0, c1, c2, c3) = history
    store = pi.RecordStore(tmp_path / "store")
    for commit in (c0, c1, c2):
        store.put(_record(commit, [_axis("x/whole", (1000,) * 5)]))
    assert store.nearest_before(repo, c3, DIGEST)["commit"] == c1
    payload = pi.verdict(store, repo, _record(c3, [_axis("x/whole", (1050,) * 5)]))
    assert payload["creep"]["base"] == c1
    assert [row["axis"] for row in payload["creep"]["regressed"]] == ["x/whole"]


def test_run_answers_from_the_stored_record_without_measuring(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A second call for (commit, identity) returns early (design invariant 7)."""
    monkeypatch.setattr(pi, "select_counter", lambda request: pi.Counter.PERF)

    def measured(*args: object, **kwargs: object) -> dict[str, Any]:
        raise AssertionError("an existing record must not be remeasured")

    monkeypatch.setattr(pi, "measure_record", measured)
    commit = pi._git(pi.ROOT, "rev-parse", "HEAD")
    identity = pi.run_identity(pi.Counter.PERF, pi.Tier.HOURLY)
    record = {
        **_record(commit, [_axis("x/whole", (1000,) * 5)]),
        "identity": identity,
        "identity_digest": pi.identity_digest(identity),
    }
    store = pi.RecordStore(tmp_path / "store")
    store.put(record)
    out = tmp_path / "out"
    assert pi.main(["run", "--store", str(store.root), "--out-dir", str(out)]) == 0
    payload = json.loads((out / "verdict.json").read_text(encoding="utf-8"))
    assert (payload["commit"], payload["status"]) == (commit, "unarmed")
    assert json.loads((out / "record.json").read_text(encoding="utf-8")) == record

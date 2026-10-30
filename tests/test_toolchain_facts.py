"""Toolchain probe answers are keyed by every input that can change them.

HF-357: a warm `molt build` must not spawn rustc, rustup, cargo, xcrun or a
C driver to re-ask questions an unchanged toolchain already answered. Each
test counts spawns at the real `process_guard.run_completed_command` boundary
through a fake that answers like the tool, and proves that each key input
forces a new probe, that an unrelated input does not, and that a corrupt
store is discarded and rewritten.
"""

from __future__ import annotations

from dataclasses import replace

from molt.durable_file_identity import StatToken

import json
import os
import shutil
import stat
import subprocess
from pathlib import Path

import pytest

from tools.check_rust_toolchain import RUST_EDITION

from molt import platform_toolchain, process_guard, rust_toolchain
from molt import toolchain_facts as facts
from molt.cli import cargo_capabilities
from molt.cli import native_link_tool_identity as link_tools
from molt.cli import runtime_cargo_plan as plans
from molt.cli import rustc_probes
from molt.cli.cargo_resource_custody import CargoExecutableCustody
from molt.process_environment import ProcessEnvironment
from tests.executable_test_support import write_mock_executable
from tests.process_guard_common import run_guarded_test_process
from tests.rustc_test_support import rustc_target_metadata_stdout

DAY_NS = 86_400 * 1_000_000_000


def _tool(name: str, content: bytes = b"MZ tool") -> facts.ProbeTool:
    """A key tool with a fabricated generation; no file is read."""
    from molt.toolchain_identity import StableRegularFileIdentity

    identity = StableRegularFileIdentity(
        path=Path("/tools") / name,
        size=len(content),
        _stat_identity=(1, 2, stat.S_IFREG | 0o755, len(content), 3, 4),
        _content_change_time_ns=4,
        sha256=f"{hash(content) & 0xFFFFFFFF:064x}",
    )
    return facts.ProbeTool.from_identity(Path("/bin") / name, identity)


def _key(**overrides: object) -> facts.ProbeKey:
    fields: dict[str, object] = {
        "probe": "test-probe",
        "tools": (_tool("rustc"),),
        "argv": ("--print", "cfg"),
        "cwd": "/project",
        "environment": (("RUSTC_BOOTSTRAP", None), ("SDKROOT", "/sdk")),
        "inputs": (
            (
                "/sysroot/lib/librustc_driver.dylib",
                facts.FileToken(
                    StatToken(
                        2,
                        3,
                        4,
                        5,
                        6,
                    )
                ),
            ),
        ),
    }
    fields.update(overrides)
    return facts.ProbeKey(**fields)  # type: ignore[arg-type]


def _fresh_process(monkeypatch: pytest.MonkeyPatch) -> None:
    """Forget every process-local store, as a new `molt` process would."""
    monkeypatch.setattr(facts, "_STORES", {})


@pytest.fixture
def store() -> facts.ToolchainFactStore:
    return facts.host_toolchain_facts()


def _rewrite_with_restored_mtime(path: Path, value: bytes) -> None:
    """Rewrite bytes in place; only the content change time can tell."""
    before = path.stat()
    assert len(value) == before.st_size
    path.write_bytes(value)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))


@pytest.mark.parametrize("repeat", [0, 1])
def test_default_fact_store_cannot_pollute_the_host_namespace(
    tmp_path_factory: pytest.TempPathFactory,
    monkeypatch: pytest.MonkeyPatch,
    repeat: int,
) -> None:
    from concurrent.futures import ThreadPoolExecutor
    from threading import Barrier

    # Use a disposable stand-in for the host, including across these cases.
    # This law is safe even when the automatic isolation is removed.
    host = tmp_path_factory.getbasetemp() / "surrogate-host-cache"
    monkeypatch.setenv("MOLT_CACHE", str(host))
    # Existing tests undo their fake process boundary before real probes.
    # Reestablish the disposable host even if isolation itself was undone.
    monkeypatch.undo()
    monkeypatch.setenv("MOLT_CACHE", str(host))
    path = facts.toolchain_fact_store_path(host)
    path.parent.mkdir(parents=True, exist_ok=True)
    original = b"host cache must remain byte-for-byte unchanged\n"
    path.write_bytes(original)
    allocations: list[str] = []
    mktemp = tmp_path_factory.mktemp

    def counted_mktemp(basename: str, numbered: bool = True) -> Path:
        allocations.append(basename)
        return mktemp(basename, numbered=numbered)

    monkeypatch.setattr(tmp_path_factory, "mktemp", counted_mktemp)
    aliases = (
        facts.host_toolchain_facts,
        rust_toolchain.host_toolchain_facts,
        platform_toolchain.host_toolchain_facts,
        link_tools.host_toolchain_facts,
        rustc_probes.host_toolchain_facts,
        cargo_capabilities.host_toolchain_facts,
    )
    ready = Barrier(len(aliases))

    def acquire(alias):
        ready.wait(timeout=5)
        return alias()

    with ThreadPoolExecutor(max_workers=len(aliases)) as pool:
        acquired = list(pool.map(acquire, aliases))
    store = acquired[0]
    assert all(value is store for value in acquired)
    key = _key(probe="pytest-private-namespace")
    assert store.lookup(key) is None  # No previous test's process memo or disk.
    answer = facts.ProbeAnswer(f"synthetic answer {repeat}", "")
    for alias in aliases:
        assert alias() is store  # Preimported consumers keep one owner.
    store.record(key, answer, persist=True)
    assert path.read_bytes() == original
    assert store.cache_root.is_relative_to(tmp_path_factory.getbasetemp())
    _fresh_process(monkeypatch)
    assert facts.host_toolchain_facts().lookup(key) == answer
    # Explicit store owners still share real disk state at their chosen root.
    explicit = facts.ToolchainFactStore(store.cache_root, max_age_ns=DAY_NS)
    assert explicit.lookup(key) == answer
    assert allocations == ["toolchain-facts"]


# --- the store -----------------------------------------------------------


def test_persisted_answers_survive_a_new_process(
    store: facts.ToolchainFactStore, monkeypatch: pytest.MonkeyPatch
) -> None:
    key = _key()
    assert store.lookup(key) is None
    store.record(key, facts.ProbeAnswer("answer\n", ""), persist=True)
    assert store.persisted(key)
    _fresh_process(monkeypatch)
    again = facts.host_toolchain_facts()
    assert again is not store
    assert again.lookup(key) == facts.ProbeAnswer("answer\n", "")
    document = json.loads(store.path.read_text(encoding="utf-8"))
    assert document["schema"] == facts.STORE_SCHEMA
    assert list(document["facts"]) == [key.digest()]
    assert document["facts"][key.digest()]["key"] == key.as_json()


def test_provisional_answers_live_in_one_process_until_persisted(
    store: facts.ToolchainFactStore, monkeypatch: pytest.MonkeyPatch
) -> None:
    key = _key()
    store.record(key, facts.ProbeAnswer("provisional", ""), persist=False)
    assert store.lookup(key) == facts.ProbeAnswer("provisional", "")
    assert not store.persisted(key)
    assert not store.path.exists()
    _fresh_process(monkeypatch)
    assert facts.host_toolchain_facts().lookup(key) is None
    store.record(key, facts.ProbeAnswer("qualified", ""), persist=True)
    assert store.persisted(key)
    assert store.lookup(key) == facts.ProbeAnswer("qualified", "")
    store.record(key, facts.ProbeAnswer("later", ""), persist=False)
    store.forget(key)
    assert store.lookup(key) == facts.ProbeAnswer("qualified", "")


@pytest.mark.parametrize(
    "damage",
    ["garbage", "schema", "facts-shape", "record-shape", "tampered-key"],
)
def test_corrupt_or_foreign_store_is_discarded_and_rewritten(
    store: facts.ToolchainFactStore, damage: str
) -> None:
    key = _key()
    kept = _key(argv=("--print", "sysroot"))
    store.record(kept, facts.ProbeAnswer("kept", ""), persist=True)
    document = json.loads(store.path.read_text(encoding="utf-8"))
    if damage == "garbage":
        store.path.write_bytes(b"\x00{not json")
    elif damage == "schema":
        document["schema"] = "molt.toolchain-facts.v0"
        store.path.write_text(json.dumps(document), encoding="utf-8")
    elif damage == "facts-shape":
        document["facts"] = []
        store.path.write_text(json.dumps(document), encoding="utf-8")
    elif damage == "record-shape":
        document["facts"][key.digest()] = {"stdout": "x"}
        store.path.write_text(json.dumps(document), encoding="utf-8")
    else:
        # A record whose key no longer hashes to its digest answers nothing,
        # not even the question its forged key names.
        document["facts"][kept.digest()]["key"] = key.as_json()
        store.path.write_text(json.dumps(document), encoding="utf-8")
    # Only a malformed sibling leaves the valid record in place; every other
    # damage discards the file or the record itself.
    kept_survives = damage == "record-shape"
    fresh = facts.ToolchainFactStore(store.cache_root, max_age_ns=30 * DAY_NS)
    assert fresh.lookup(key) is None
    assert fresh.persisted(kept) == kept_survives
    fresh.record(key, facts.ProbeAnswer("new", ""), persist=True)
    rewritten = json.loads(fresh.path.read_text(encoding="utf-8"))
    assert rewritten["schema"] == facts.STORE_SCHEMA
    assert rewritten["facts"][key.digest()]["stdout"] == "new"
    assert (kept.digest() in rewritten["facts"]) == kept_survives


def test_aged_records_are_dropped_and_used_records_refreshed(
    store: facts.ToolchainFactStore,
) -> None:
    used = _key(argv=("used",))
    stale = _key(argv=("stale",))
    old = _key(argv=("old",))
    store.record(used, facts.ProbeAnswer("u", ""), persist=True)
    store.record(stale, facts.ProbeAnswer("s", ""), persist=True)
    store.record(old, facts.ProbeAnswer("o", ""), persist=True)
    document = json.loads(store.path.read_text(encoding="utf-8"))
    now = document["facts"][used.digest()]["recorded_ns"]
    document["facts"][used.digest()]["recorded_ns"] = now - 20 * DAY_NS
    document["facts"][stale.digest()]["recorded_ns"] = now - 20 * DAY_NS
    document["facts"][old.digest()]["recorded_ns"] = now - 40 * DAY_NS
    store.path.write_text(json.dumps(document), encoding="utf-8")
    fresh = facts.ToolchainFactStore(store.cache_root, max_age_ns=30 * DAY_NS)
    # A hit in the second half of the budget refreshes the record; that write
    # drops records past the budget and keeps unused ones inside it.
    assert fresh.lookup(used) == facts.ProbeAnswer("u", "")
    rewritten = json.loads(fresh.path.read_text(encoding="utf-8"))["facts"]
    assert rewritten[used.digest()]["recorded_ns"] >= now
    assert rewritten[stale.digest()]["recorded_ns"] == now - 20 * DAY_NS
    assert old.digest() not in rewritten
    assert fresh.lookup(old) is None


def test_parallel_writers_merge_instead_of_overwriting(
    store: facts.ToolchainFactStore,
) -> None:
    first = facts.ToolchainFactStore(store.cache_root, max_age_ns=30 * DAY_NS)
    second = facts.ToolchainFactStore(store.cache_root, max_age_ns=30 * DAY_NS)
    one, two = _key(argv=("one",)), _key(argv=("two",))
    assert first.lookup(one) is None and second.lookup(two) is None
    first.record(one, facts.ProbeAnswer("1", ""), persist=True)
    second.record(two, facts.ProbeAnswer("2", ""), persist=True)
    third = facts.ToolchainFactStore(store.cache_root, max_age_ns=30 * DAY_NS)
    assert third.lookup(one) == facts.ProbeAnswer("1", "")
    assert third.lookup(two) == facts.ProbeAnswer("2", "")
    assert second.lookup(one) == facts.ProbeAnswer("1", "")


def test_completed_probe_keeps_only_zero_exit_and_accepted_answers(
    store: facts.ToolchainFactStore,
) -> None:
    key = _key()
    outputs = iter(
        [
            subprocess.CompletedProcess(["x"], 1, "", "broken"),
            subprocess.CompletedProcess(["x"], 0, "stale\n", ""),
            subprocess.CompletedProcess(["x"], 0, "fresh\n", ""),
        ]
    )
    runs = 0

    def run() -> subprocess.CompletedProcess[str]:
        nonlocal runs
        runs += 1
        return next(outputs)

    failed, cached = facts.completed_probe(store, key, run)
    assert failed.returncode == 1 and not cached and store.lookup(key) is None
    stale, cached = facts.completed_probe(store, key, run)
    assert stale.stdout == "stale\n" and not cached
    hit, cached = facts.completed_probe(store, key, run)
    assert hit.stdout == "stale\n" and cached and runs == 2
    assert hit.args == list(key.argv)
    fresh, cached = facts.completed_probe(
        store, key, run, accept=lambda answer: answer.stdout != "stale\n"
    )
    assert fresh.stdout == "fresh\n" and not cached and runs == 3
    assert store.lookup(key) == facts.ProbeAnswer("fresh\n", "")
    with pytest.raises(TypeError, match="text"):
        facts.completed_probe(
            store,
            _key(argv=("bytes",)),
            lambda: subprocess.CompletedProcess(["x"], 0, b"", b""),  # type: ignore[arg-type]
        )


# --- the key ---------------------------------------------------------------


@pytest.mark.parametrize(
    "mutation",
    [
        "probe",
        "tool-entrypoint",
        "tool-content-path",
        "tool-size",
        "tool-sha256",
        "tool-token",
        "tool-added",
        "argv",
        "cwd",
        "environment-value",
        "environment-absent",
        "environment-name",
        "input-token",
        "input-absent",
        "input-path",
        "input-added",
    ],
)
def test_each_key_input_changes_the_digest(mutation: str) -> None:
    base = _key()
    tool = base.tools[0]
    identity = tool.identity
    changed: dict[str, object] = {}
    if mutation == "probe":
        changed["probe"] = "other-probe"
    elif mutation == "tool-entrypoint":
        changed["tools"] = (
            facts.ProbeTool.from_identity(Path("/other/rustc"), identity),
        )
    elif mutation == "tool-content-path":
        changed["tools"] = (
            facts.ProbeTool(
                tool.entrypoint,
                "/elsewhere/rustc",
                tool.size,
                tool.sha256,
                tool.token,
                identity,
            ),
        )
    elif mutation == "tool-size":
        changed["tools"] = (
            facts.ProbeTool(
                tool.entrypoint,
                tool.content_path,
                tool.size + 1,
                tool.sha256,
                tool.token,
                identity,
            ),
        )
    elif mutation == "tool-sha256":
        changed["tools"] = (
            facts.ProbeTool(
                tool.entrypoint,
                tool.content_path,
                tool.size,
                "0" * 64,
                tool.token,
                identity,
            ),
        )
    elif mutation == "tool-token":
        token = replace(tool.token, ctime_ns=tool.token.ctime_ns + 1)
        changed["tools"] = (
            facts.ProbeTool(
                tool.entrypoint,
                tool.content_path,
                tool.size,
                tool.sha256,
                token,
                identity,
            ),
        )
    elif mutation == "tool-added":
        changed["tools"] = (_tool("sccache"), tool)
    elif mutation == "argv":
        changed["argv"] = ("--print", "sysroot")
    elif mutation == "cwd":
        changed["cwd"] = "/other"
    elif mutation == "environment-value":
        changed["environment"] = (("RUSTC_BOOTSTRAP", None), ("SDKROOT", "/other"))
    elif mutation == "environment-absent":
        changed["environment"] = (("RUSTC_BOOTSTRAP", None), ("SDKROOT", None))
    elif mutation == "environment-name":
        changed["environment"] = (("RUSTC_BOOTSTRAP", "1"), ("SDKROOT", "/sdk"))
    elif mutation == "input-token":
        changed["inputs"] = (
            (
                "/sysroot/lib/librustc_driver.dylib",
                facts.FileToken(
                    StatToken(
                        2,
                        3,
                        4,
                        5,
                        7,
                    )
                ),
            ),
        )
    elif mutation == "input-absent":
        changed["inputs"] = (("/sysroot/lib/librustc_driver.dylib", None),)
    elif mutation == "input-path":
        changed["inputs"] = (
            (
                "/sysroot/lib/other.dylib",
                facts.FileToken(
                    StatToken(
                        2,
                        3,
                        4,
                        5,
                        6,
                    )
                ),
            ),
        )
    else:
        changed["inputs"] = (*base.inputs, ("/sysroot/lib/target.json", None))
    assert _key(**changed).digest() != base.digest()
    # The same inputs, built again, are the same key: a stable digest is what
    # makes a hit possible at all.
    assert _key().digest() == base.digest()
    assert _key(**changed).digest() == _key(**changed).digest()


def test_file_and_directory_inputs_name_absence_links_and_listings(
    tmp_path: Path,
) -> None:
    missing = facts.file_input(tmp_path / "absent")
    assert missing == (os.fspath(tmp_path / "absent"), None)
    regular = tmp_path / "lib.dylib"
    regular.write_bytes(b"lib")
    name, token = facts.file_input(regular)
    assert (
        name == os.fspath(regular) and token is not None and len(token.payload()) == 5
    )
    assert token.node.size == 3 and stat.S_ISREG(token.node.mode)
    if os.name != "nt":
        link = tmp_path / "link"
        link.symlink_to(regular)
        _name, link_token = facts.file_input(link)
        assert link_token is not None and link_token.link_text == os.fspath(regular)
        assert stat.S_ISLNK(link_token.node.mode)
        # A probe reads through the link: an edit behind it changes the key.
        _rewrite_with_restored_mtime(regular, b"LIB")
        regular.write_bytes(b"libs")
        _name, edited = facts.file_input(link)
        assert (
            edited is not None
            and edited.node == link_token.node
            and edited.link_text == link_token.link_text
        )
        assert edited.target != link_token.target
        dangling = tmp_path / "dangling"
        dangling.symlink_to(tmp_path / "gone")
        assert facts.file_input(dangling)[1].target is None
    (tmp_path / "sub").mkdir()
    (tmp_path / "notes.txt").write_bytes(b"text")
    listing = facts.directory_inputs(tmp_path)
    assert listing[0][0] == os.fspath(tmp_path)
    names = [Path(path).name for path, _token in listing[1:]]
    assert "sub" not in names and "lib.dylib" in names and "notes.txt" in names
    dynamic = facts.directory_inputs(tmp_path, dynamic_libraries_only=True)
    assert [Path(path).name for path, _token in dynamic[1:]] == ["lib.dylib"]
    assert facts.directory_inputs(tmp_path / "absent") == (
        (os.fspath(tmp_path / "absent"), None),
    )


def test_environment_inputs_follow_the_captured_name_rule() -> None:
    from molt.process_environment import NameRule

    values = ProcessEnvironment({"Sdkroot": "/sdk"}, rule=NameRule.WINDOWS)
    assert facts.environment_inputs(values, ("SDKROOT", "DEVELOPER_DIR")) == (
        ("DEVELOPER_DIR", None),
        ("SDKROOT", "/sdk"),
    )
    assert facts.environment_inputs({"Sdkroot": "/sdk"}, ("SDKROOT",)) == (
        ("SDKROOT", "/sdk" if os.name == "nt" else None),
    )


# --- rustc target metadata ----------------------------------------------------


class _Spawns:
    """A fake at the process_guard boundary that answers like the tool."""

    def __init__(self, answer) -> None:  # type: ignore[no-untyped-def]
        self.answer = answer
        self.commands: list[list[str]] = []
        self.environments: list[dict[str, str]] = []

    def __call__(self, cmd, **kwargs):  # type: ignore[no-untyped-def]
        command = [str(part) for part in cmd]
        self.commands.append(command)
        self.environments.append(dict(kwargs.get("env") or {}))
        stdout, stderr = self.answer(command, kwargs)
        return subprocess.CompletedProcess(command, 0, stdout, stderr)

    @property
    def count(self) -> int:
        return len(self.commands)


def _rust_toolchain(root: Path) -> tuple[Path, Path]:
    """A launcher beside its driver library, as a sysroot lays them out."""
    sysroot = root / "toolchain"
    (sysroot / "bin").mkdir(parents=True)
    (sysroot / "lib").mkdir()
    rustc = write_mock_executable(sysroot / "bin" / "rustc", b"MZ rustc launcher")
    (sysroot / "lib" / "librustc_driver.dylib").write_bytes(b"driver generation 1")
    return sysroot, rustc


def _metadata_spawns(sysroot: Path) -> _Spawns:
    return _Spawns(
        lambda command, kwargs: (
            rustc_target_metadata_stdout(sysroot, 'unix\ntarget_arch="aarch64"\n'),
            "",
        )
    )


def test_rustc_metadata_is_asked_once_per_toolchain_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sysroot, rustc = _rust_toolchain(tmp_path)
    wrapper = write_mock_executable(tmp_path / "sccache", b"MZ wrapper")
    wrappers = {"RUSTC_WRAPPER": wrapper}
    spawns = _metadata_spawns(sysroot)
    monkeypatch.setattr(process_guard, "run_completed_command", spawns)
    root = tmp_path / "project"
    root.mkdir()
    env = {"RUSTC_LOG": "debug"}

    def ask(**kwargs: object) -> plans.RustcTargetMetadata:
        return rustc_probes.rust_target_metadata(
            rustc,
            "aarch64-apple-darwin",
            ("--cfg", "x"),
            root=root,
            env=env,
            wrappers=wrappers,
            **kwargs,  # type: ignore[arg-type]
        )

    discovered = ask()
    assert spawns.count == 1
    assert spawns.commands[0][:2] == [os.fspath(wrapper), os.fspath(rustc)]
    assert "RUSTC_LOG" not in spawns.environments[0]
    assert ("unix", None) in discovered.cfg
    # Discovery is provisional: a second discovery in this process asks the
    # store, and a new process would ask rustc again.
    assert ask() == discovered and spawns.count == 1
    _fresh_process(monkeypatch)
    assert ask() == discovered and spawns.count == 2
    # Qualification replays a provisional answer, then persists it.
    assert ask(qualify=True) == discovered and spawns.count == 3
    _fresh_process(monkeypatch)
    assert ask() == discovered and spawns.count == 3
    assert ask(qualify=True) == discovered and spawns.count == 3


@pytest.mark.parametrize(
    "change",
    [
        "rustc-bytes",
        "wrapper-bytes",
        "driver-library",
        "target-spec-appears",
        "environment",
        "flags",
        "cwd",
        "unrelated-environment",
    ],
)
def test_rustc_metadata_reprobes_when_a_key_input_changes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str,
) -> None:
    sysroot, rustc = _rust_toolchain(tmp_path)
    wrapper = write_mock_executable(tmp_path / "sccache", b"MZ wrapper")
    wrappers = {"RUSTC_WRAPPER": wrapper}
    spawns = _metadata_spawns(sysroot)
    monkeypatch.setattr(process_guard, "run_completed_command", spawns)
    root = tmp_path / "project"
    root.mkdir()
    specs = tmp_path / "specs"
    specs.mkdir()
    env = {"RUST_TARGET_PATH": os.fspath(specs), "TERM": "dumb"}
    flags: tuple[str, ...] = ("--cfg", "x")

    def ask() -> plans.RustcTargetMetadata:
        return rustc_probes.rust_target_metadata(
            rustc,
            "custom-target",
            flags,
            root=root,
            env=env,
            wrappers=wrappers,
            qualify=True,
        )

    ask()
    assert spawns.count == 1
    _fresh_process(monkeypatch)
    ask()
    assert spawns.count == 1
    if change == "rustc-bytes":
        _rewrite_with_restored_mtime(rustc, b"MZ rustc launchez")
    elif change == "wrapper-bytes":
        _rewrite_with_restored_mtime(wrapper, b"MZ wrappez")
    elif change == "driver-library":
        _rewrite_with_restored_mtime(
            sysroot / "lib" / "librustc_driver.dylib", b"driver generation 2"
        )
    elif change == "target-spec-appears":
        (specs / "custom-target.json").write_text("{}", encoding="utf-8")
    elif change == "environment":
        env["RUSTC_BOOTSTRAP"] = "1"
    elif change == "flags":
        flags = ("--cfg", "y")
    elif change == "cwd":
        root = tmp_path / "other"
        root.mkdir()
    else:
        env["TERM"] = "xterm"
    _fresh_process(monkeypatch)
    ask()
    assert spawns.count == (1 if change == "unrelated-environment" else 2)


def test_rustc_metadata_qualification_persists_only_an_agreeing_answer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sysroot, rustc = _rust_toolchain(tmp_path)
    answers = iter(["unix\nfirst\n", "unix\nsecond\n", "unix\nsecond\n"])
    spawns = _Spawns(
        lambda command, kwargs: (
            rustc_target_metadata_stdout(sysroot, next(answers)),
            "",
        )
    )
    monkeypatch.setattr(process_guard, "run_completed_command", spawns)
    root = tmp_path / "project"
    root.mkdir()

    def ask(qualify: bool) -> plans.RustcTargetMetadata:
        return rustc_probes.rust_target_metadata(
            rustc, None, (), root=root, env={}, qualify=qualify
        )

    assert ("first", None) in ask(False).cfg
    # The replay disagrees: nothing is persisted and the provisional answer is
    # dropped, so the plan's own comparison fails closed and the next process
    # asks rustc again.
    assert ("second", None) in ask(True).cfg
    assert spawns.count == 2
    assert not facts.host_toolchain_facts().path.exists()
    _fresh_process(monkeypatch)
    assert ("second", None) in ask(False).cfg
    assert spawns.count == 3


def test_rustc_metadata_records_nothing_when_the_probe_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sysroot, rustc = _rust_toolchain(tmp_path)
    calls = 0

    def run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        nonlocal calls
        calls += 1
        if calls == 1:
            return subprocess.CompletedProcess(list(cmd), 1, "", "unsupported target")
        if calls == 2:
            _rewrite_with_restored_mtime(rustc, b"MZ rustc launchez")
        return subprocess.CompletedProcess(
            list(cmd), 0, rustc_target_metadata_stdout(sysroot), ""
        )

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    root = tmp_path / "project"
    root.mkdir()
    with pytest.raises(ValueError, match="unsupported target"):
        rustc_probes.rust_target_metadata(
            rustc, None, (), root=root, env={}, qualify=True
        )
    # A compiler replaced while it answered is not the key's generation.
    with pytest.raises(ValueError, match="changed"):
        rustc_probes.rust_target_metadata(
            rustc, None, (), root=root, env={}, qualify=True
        )
    assert calls == 2
    assert not facts.host_toolchain_facts().path.exists()


# --- rustup which --------------------------------------------------------------


def _rustup_setup(root: Path) -> tuple[Path, Path, Path]:
    bin_dir = root / "cargo-home" / "bin"
    bin_dir.mkdir(parents=True)
    rustup = write_mock_executable(bin_dir / "rustup", b"MZ rustup proxy")
    proxy = write_mock_executable(bin_dir / "rustc", b"MZ rustup proxy")
    toolchain = root / "rustup-home" / "toolchains" / "stable" / "bin"
    toolchain.mkdir(parents=True)
    selected = write_mock_executable(toolchain / "rustc", b"MZ real rustc")
    return proxy, rustup, selected


@pytest.mark.parametrize(
    "change",
    [
        "toolchain-file",
        "rustup-toolchain",
        "settings",
        "toolchain-installed",
        "component-installed",
        "selected-removed",
        "role",
        "unrelated-environment",
    ],
)
def test_rustup_selection_is_asked_once_per_selection_inputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str,
) -> None:
    proxy, rustup, selected = _rustup_setup(tmp_path)
    spawns = _Spawns(lambda command, kwargs: (os.fspath(selected) + "\n", ""))
    monkeypatch.setattr(process_guard, "run_completed_command", spawns)
    root = tmp_path / "project"
    root.mkdir()
    env = {
        "RUSTUP_HOME": os.fspath(tmp_path / "rustup-home"),
        "HOME": os.fspath(tmp_path),
    }
    role = "rustc"

    def resolve() -> Path:
        return rust_toolchain.resolve_rustup_proxy(proxy, role=role, root=root, env=env)

    assert resolve() == selected
    assert spawns.count == 1
    assert spawns.commands[0] == [os.fspath(rustup), "which", "rustc"]
    _fresh_process(monkeypatch)
    assert resolve() == selected and spawns.count == 1
    if change == "toolchain-file":
        (root / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.99.0"\n', encoding="utf-8"
        )
    elif change == "rustup-toolchain":
        env["RUSTUP_TOOLCHAIN"] = "nightly"
    elif change == "settings":
        (tmp_path / "rustup-home" / "settings.toml").write_text(
            "version = '12'\n", encoding="utf-8"
        )
    elif change == "toolchain-installed":
        (tmp_path / "rustup-home" / "toolchains" / "nightly").mkdir()
    elif change == "component-installed":
        write_mock_executable(selected.with_name("cargo"), b"MZ cargo")
    elif change == "selected-removed":
        selected.unlink()
        selected = write_mock_executable(selected.with_name("rustc-new"), b"MZ new")
        spawns.answer = lambda command, kwargs: (os.fspath(selected) + "\n", "")
    elif change == "role":
        role = "cargo"
    else:
        env["TERM"] = "xterm"
    _fresh_process(monkeypatch)
    assert resolve() == selected
    assert spawns.count == (1 if change == "unrelated-environment" else 2)


@pytest.mark.parametrize(
    "change",
    [
        "parent-toolchain-file",
        "legacy-toolchain-file",
        "symlinked-toolchain-target",
        "physical-parent",
        "spelled-parent",
        pytest.param(
            "fallback-settings",
            marks=pytest.mark.skipif(os.name == "nt", reason="Unix fallback only"),
        ),
        "relative-rustup-home",
    ],
)
def test_rustup_selection_keys_what_rustup_reads(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str,
) -> None:
    # Each case changes one input real rustup 1.29 reads while every other
    # input stays the same; the store must ask rustup again.
    proxy, _rustup, selected = _rustup_setup(tmp_path)
    spawns = _Spawns(lambda command, kwargs: (os.fspath(selected) + "\n", ""))
    monkeypatch.setattr(process_guard, "run_completed_command", spawns)
    physical = tmp_path / "physical" / "project"
    physical.mkdir(parents=True)
    root = physical
    env = {
        "RUSTUP_HOME": os.fspath(tmp_path / "rustup-home"),
        "HOME": os.fspath(tmp_path),
        "RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS": os.fspath(tmp_path / "fallback.toml"),
    }
    if change == "symlinked-toolchain-target":
        if os.name == "nt":
            pytest.skip("POSIX symlinks")
        (tmp_path / "shared.toml").write_text(
            '[toolchain]\nchannel = "a"\n', encoding="utf-8"
        )
        (physical / "rust-toolchain.toml").symlink_to(tmp_path / "shared.toml")
    elif change == "physical-parent":
        if os.name == "nt":
            pytest.skip("POSIX symlinks")
        root = tmp_path / "alias"
        root.symlink_to(physical, target_is_directory=True)
    elif change == "spelled-parent":
        # Windows rustup walks the path as spelled (a junction or substituted
        # drive keeps it): the alias's own parent is on no physical chain.
        if os.name == "nt":
            pytest.skip("POSIX symlinks")
        (tmp_path / "aliases").mkdir()
        root = tmp_path / "aliases" / "alias"
        root.symlink_to(physical, target_is_directory=True)
    elif change == "relative-rustup-home":
        shutil.copytree(tmp_path / "rustup-home", physical / "rel-home")
        env["RUSTUP_HOME"] = "rel-home"

    def resolve() -> Path:
        return rust_toolchain.resolve_rustup_proxy(
            proxy, role="rustc", root=root, env=env
        )

    assert resolve() == selected and spawns.count == 1
    _fresh_process(monkeypatch)
    assert resolve() == selected and spawns.count == 1
    if change == "parent-toolchain-file":
        (physical.parent / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "b"\n', encoding="utf-8"
        )
    elif change == "legacy-toolchain-file":
        (physical / "rust-toolchain").write_text("nightly\n", encoding="utf-8")
    elif change == "symlinked-toolchain-target":
        (tmp_path / "shared.toml").write_text(
            '[toolchain]\nchannel = "bb"\n', encoding="utf-8"
        )
    elif change == "physical-parent":
        # Only the physical walk sees this file; the alias's parent is tmp_path.
        (physical.parent / "rust-toolchain").write_text("nightly\n", encoding="utf-8")
    elif change == "spelled-parent":
        (tmp_path / "aliases" / "rust-toolchain").write_text(
            "nightly\n", encoding="utf-8"
        )
    elif change == "fallback-settings":
        (tmp_path / "fallback.toml").write_text(
            "default_toolchain = 'nightly'\n", encoding="utf-8"
        )
    else:
        (physical / "rel-home" / "settings.toml").write_text(
            "version = '12'\n", encoding="utf-8"
        )
    _fresh_process(monkeypatch)
    assert resolve() == selected
    assert spawns.count == 2


@pytest.mark.parametrize("windows", [False, True])
def test_rustup_home_follows_the_home_crate(tmp_path: Path, windows: bool) -> None:
    cwd = tmp_path / "project"
    relative = {"RUSTUP_HOME": "rel", "HOME": "/h", "USERPROFILE": "/u"}
    assert rust_toolchain.rustup_home(relative, cwd, windows=windows) == cwd / "rel"
    absolute = {"RUSTUP_HOME": os.fspath(tmp_path / "abs")}
    assert (
        rust_toolchain.rustup_home(absolute, cwd, windows=windows) == tmp_path / "abs"
    )
    # An empty RUSTUP_HOME is unset; Windows reads USERPROFILE, never HOME.
    homes = {"RUSTUP_HOME": "", "HOME": "/h", "USERPROFILE": "/u"}
    expected = Path("/u" if windows else "/h") / ".rustup"
    assert rust_toolchain.rustup_home(homes, cwd, windows=windows) == expected
    assert rust_toolchain.rustup_fallback_settings({}, cwd, windows=windows) == (
        None if windows else Path("/etc/rustup/settings.toml")
    )
    relative = {"RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS": "fb.toml"}
    assert rust_toolchain.rustup_fallback_settings(relative, cwd, windows=windows) == (
        None if windows else cwd / "fb.toml"
    )


def test_no_home_is_absent_not_an_error(monkeypatch: pytest.MonkeyPatch) -> None:
    import pwd

    def no_entry(_uid):
        raise KeyError("getpwuid(): uid not found")

    monkeypatch.setattr(pwd, "getpwuid", no_entry)
    # The `home` crate returns None here; rustup then reports the error itself.
    assert rust_toolchain.rustup_home({}, Path("/p"), windows=False) is None


def _real_rustup() -> Path | None:
    found = shutil.which("rustup") or os.fspath(Path.home() / ".cargo/bin/rustup")
    path = Path(found)
    proxy = path.with_name("rustc")
    if not (path.is_file() and proxy.exists()):
        return None
    # The rustc beside rustup must be a rustup proxy, or there is nothing to ask.
    return path if rust_toolchain.rustup_proxy_selection(proxy) is not None else None


@pytest.mark.skipif(
    _real_rustup() is None or os.name == "nt", reason="needs a real rustup on POSIX"
)
@pytest.mark.parametrize(
    "change",
    [
        "parent-toolchain-file",
        "legacy-toolchain-file",
        "symlinked-toolchain-target",
        "physical-parent",
        "fallback-settings",
        "relative-fallback-settings",
        "relative-rustup-home",
    ],
)
def test_store_answers_what_real_rustup_answers(
    tmp_path: Path,
    isolated_molt_cache: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str,
) -> None:
    # The oracle is rustup itself, not this file's list of its inputs: an
    # input the key misses shows up as the store answering the old toolchain.
    rustup = _real_rustup()
    assert rustup is not None
    for name in ("a", "b"):
        bin_dir = tmp_path / f"tc-{name}" / "bin"
        bin_dir.mkdir(parents=True)
        write_mock_executable(bin_dir / "rustc", b"#!/bin/sh\n")
    home = tmp_path / "rustup-home"
    (home / "toolchains").mkdir(parents=True)
    for name in ("a", "b"):
        (home / "toolchains" / name).symlink_to(tmp_path / f"tc-{name}")
    physical = tmp_path / "physical" / "project"
    physical.mkdir(parents=True)
    root = physical
    env = {
        "RUSTUP_HOME": os.fspath(home),
        "HOME": os.fspath(tmp_path / "home"),
        "RUSTUP_AUTO_INSTALL": "0",
        "PATH": "/usr/bin:/bin",
        "RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS": os.fspath(tmp_path / "none.toml"),
        "MOLT_EXT_ROOT": os.fspath(tmp_path),
    }

    def toolchain_file(path: Path, name: str, *, legacy: bool = False) -> None:
        text = f"{name}\n" if legacy else f'[toolchain]\nchannel = "{name}"\n'
        path.write_text(text, encoding="utf-8")

    if change == "parent-toolchain-file":
        toolchain_file(physical.parent / "rust-toolchain.toml", "a")
    elif change == "legacy-toolchain-file":
        toolchain_file(physical.parent / "rust-toolchain.toml", "a")
    elif change == "symlinked-toolchain-target":
        toolchain_file(tmp_path / "shared.toml", "a")
        (physical / "rust-toolchain.toml").symlink_to(tmp_path / "shared.toml")
    elif change == "physical-parent":
        toolchain_file(tmp_path / "rust-toolchain.toml", "a")
        root = tmp_path / "alias"
        root.symlink_to(physical, target_is_directory=True)
    elif change == "fallback-settings":
        env["RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS"] = os.fspath(tmp_path / "fb.toml")
        (tmp_path / "fb.toml").write_text('default_toolchain = "a"\n', encoding="utf-8")
    elif change == "relative-fallback-settings":
        env["RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS"] = "fb.toml"
        (physical / "fb.toml").write_text('default_toolchain = "a"\n', encoding="utf-8")
    else:
        shutil.copytree(home, physical / "rel-home", symlinks=True)
        env["RUSTUP_HOME"] = "rel-home"
        (physical / "rel-home" / "settings.toml").write_text(
            'version = "12"\ndefault_toolchain = "a"\n', encoding="utf-8"
        )

    def via_store() -> Path:
        return rust_toolchain.resolve_rustup_proxy(
            rustup.with_name("rustc"), role="rustc", root=root, env=env
        )

    def direct() -> Path:
        completed = run_guarded_test_process(
            [os.fspath(rustup), "which", "rustc"], cwd=root, env=env, timeout=60
        )
        assert completed.returncode == 0, completed.stderr
        return Path(completed.stdout.strip())

    # rustup prints <home>/toolchains/<name>/bin/rustc for the selected one.
    assert direct().parent.parent.name == "a"
    assert via_store() == direct()
    if change == "parent-toolchain-file":
        toolchain_file(physical.parent / "rust-toolchain.toml", "b")
    elif change == "legacy-toolchain-file":
        toolchain_file(physical / "rust-toolchain", "b", legacy=True)
    elif change == "symlinked-toolchain-target":
        toolchain_file(tmp_path / "shared.toml", "b")
    elif change == "physical-parent":
        toolchain_file(tmp_path / "physical" / "rust-toolchain.toml", "b")
    elif change == "fallback-settings":
        (tmp_path / "fb.toml").write_text('default_toolchain = "b"\n', encoding="utf-8")
    elif change == "relative-fallback-settings":
        (physical / "fb.toml").write_text('default_toolchain = "b"\n', encoding="utf-8")
    else:
        (physical / "rel-home" / "settings.toml").write_text(
            'version = "12"\ndefault_toolchain = "b"\n', encoding="utf-8"
        )
    _fresh_process(monkeypatch)
    answer = direct()
    assert answer.parent.parent.name == "b"
    assert via_store() == answer


def test_rustup_failure_is_not_recorded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    proxy, _rustup, _selected = _rustup_setup(tmp_path)
    calls = 0

    def run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        nonlocal calls
        calls += 1
        return subprocess.CompletedProcess(list(cmd), 1, "", "toolchain not installed")

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    root = tmp_path / "project"
    root.mkdir()
    for _turn in range(2):
        with pytest.raises(
            rust_toolchain.RustupProxyUnavailable, match="not installed"
        ):
            rust_toolchain.resolve_rustup_proxy(proxy, role="rustc", root=root, env={})
    assert calls == 2


# --- cargo --version -----------------------------------------------------------


@pytest.mark.parametrize(
    ("version", "build_directory", "warnings_deny"),
    [
        ("1.90.0", False, False),
        ("1.91.0", True, False),
        ("1.96.0", True, False),
        ("1.97.0", True, True),
    ],
)
@pytest.mark.parametrize("verbose", [False, True])
def test_cargo_capabilities_keep_their_independent_version_boundaries(
    version: str, build_directory: bool, warnings_deny: bool, verbose: bool
) -> None:
    banner = f"cargo {version} (abc 2026-01-01)\n"
    if verbose:
        banner += f"release: {version}\nhost: aarch64-apple-darwin\n"
    for capability, accepted, minimum in (
        (cargo_capabilities.CargoCapability.BUILD_DIRECTORY, build_directory, "1.91"),
        (cargo_capabilities.CargoCapability.WARNINGS_DENY, warnings_deny, "1.97"),
    ):
        if accepted:
            cargo_capabilities.require_cargo_capability_version(banner, capability)
        else:
            with pytest.raises(ValueError, match=minimum.replace(".", r"\.")):
                cargo_capabilities.require_cargo_capability_version(banner, capability)


def test_cargo_version_is_asked_once_per_cargo_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cargo = write_mock_executable(tmp_path / "cargo", b"MZ cargo 1.96.0")
    spawns = _Spawns(lambda command, kwargs: ("cargo 1.96.0 (abc 2026-01-01)\n", ""))
    monkeypatch.setattr(
        cargo_capabilities.process_guard, "run_completed_command", spawns
    )
    root = tmp_path / "project"
    root.mkdir()

    def admit(
        capabilities: tuple[cargo_capabilities.CargoCapability, ...] = (),
    ) -> None:
        custody = CargoExecutableCustody.capture("tool/cargo", cargo)
        cargo_capabilities.require_cargo_capabilities(
            custody, root=root, env={}, capabilities=capabilities
        )

    admit()
    assert spawns.count == 1 and spawns.commands[0] == [os.fspath(cargo), "--version"]
    # Cache the tool's answer, not a successful verdict for every capability.
    with pytest.raises(ValueError, match=r"1\.97"):
        admit((cargo_capabilities.CargoCapability.WARNINGS_DENY,))
    assert spawns.count == 1
    _fresh_process(monkeypatch)
    admit()
    with pytest.raises(ValueError, match=r"1\.97"):
        admit((cargo_capabilities.CargoCapability.WARNINGS_DENY,))
    assert spawns.count == 1
    spawns.answer = lambda command, kwargs: ("cargo 1.99.1\n", "")
    _rewrite_with_restored_mtime(cargo, b"MZ cargo 1.99.1")
    _fresh_process(monkeypatch)
    admit((cargo_capabilities.CargoCapability.WARNINGS_DENY,))
    assert spawns.count == 2
    spawns.answer = lambda command, kwargs: ("cargo 1.90.0\n", "")
    _rewrite_with_restored_mtime(cargo, b"MZ cargo 1.90.0")
    with pytest.raises(ValueError, match="1.91"):
        admit()
    assert spawns.count == 3


# --- Darwin selectors ------------------------------------------------------------


def _developer_directory(root: Path) -> Path:
    developer = root / "Developer"
    (developer / "usr" / "bin").mkdir(parents=True)
    write_mock_executable(developer / "usr" / "bin" / "xcrun", b"MZ real xcrun")
    return developer


def _selectors(root: Path) -> tuple[Path, dict[str, str]]:
    bin_dir = root / "bin"
    bin_dir.mkdir()
    write_mock_executable(bin_dir / "xcode-select", b"MZ xcode-select")
    write_mock_executable(bin_dir / "xcrun", b"MZ xcrun")
    return bin_dir, {"PATH": os.fspath(bin_dir), "HOME": os.fspath(root / "home")}


@pytest.mark.skipif(os.name == "nt", reason="the Darwin selectors never run on Windows")
@pytest.mark.parametrize(
    "change", ["developer-link", "directory-removed", "unrelated-environment"]
)
def test_developer_directory_selection_is_asked_once_per_selection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    change: str,
) -> None:
    _bin_dir, env = _selectors(tmp_path)
    developer = _developer_directory(tmp_path)
    link = tmp_path / "xcode_select_link"
    link.symlink_to(developer)
    monkeypatch.setattr(platform_toolchain, "DARWIN_DEVELOPER_SELECTION_LINK", link)
    spawns = _Spawns(lambda command, kwargs: (os.fspath(developer) + "\n", ""))
    monkeypatch.setattr(
        platform_toolchain.process_guard, "run_completed_command", spawns
    )
    assert platform_toolchain._darwin_developer_directory(env, developer_dir=None) == (
        developer.resolve()
    )
    assert spawns.count == 1 and spawns.commands[0][1:] == ["--print-path"]
    _fresh_process(monkeypatch)
    assert platform_toolchain._darwin_developer_directory(env, developer_dir=None)
    assert spawns.count == 1
    if change == "developer-link":
        other = _developer_directory(tmp_path / "other")
        link.unlink()
        link.symlink_to(other)
        spawns.answer = lambda command, kwargs: (os.fspath(other) + "\n", "")
        expected = other.resolve()
    elif change == "directory-removed":
        shutil.rmtree(developer)
        other = _developer_directory(tmp_path / "other")
        spawns.answer = lambda command, kwargs: (os.fspath(other) + "\n", "")
        expected = other.resolve()
    else:
        env["TERM"] = "xterm"
        expected = developer.resolve()
    _fresh_process(monkeypatch)
    assert (
        platform_toolchain._darwin_developer_directory(env, developer_dir=None)
        == expected
    )
    assert spawns.count == (1 if change == "unrelated-environment" else 2)


@pytest.mark.skipif(os.name == "nt", reason="the Darwin selectors never run on Windows")
def test_sdk_and_tool_selection_are_asked_once_per_developer_layout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _bin_dir, env = _selectors(tmp_path)
    developer = _developer_directory(tmp_path)
    sdk = developer / "SDKs" / "MacOSX26.5.sdk"
    sdk.mkdir(parents=True)
    (sdk / "SDKSettings.json").write_text('{"Version": "26.5"}', encoding="utf-8")
    for name in ("clang", "clang++", "ar", "ranlib", "ld"):
        write_mock_executable(developer / "usr" / "bin" / name, b"MZ " + name.encode())
    monkeypatch.setattr(
        platform_toolchain, "DARWIN_DEVELOPER_SELECTION_LINK", tmp_path / "no-link"
    )

    def answer(command: list[str], kwargs: dict[str, object]) -> tuple[str, str]:
        if command[-1] == "--show-sdk-path":
            return os.fspath(sdk) + "\n", ""
        assert command[-2] == "--find"
        return os.fspath(developer / "usr" / "bin" / command[-1]) + "\n", ""

    spawns = _Spawns(answer)
    monkeypatch.setattr(
        platform_toolchain.process_guard, "run_completed_command", spawns
    )
    env["DEVELOPER_DIR"] = os.fspath(developer)
    selected = platform_toolchain.select_darwin_sdk(env)
    assert selected.sdk_root == sdk.resolve() and selected.sdk_version == "26.5"
    assert spawns.count == 1
    toolchain = platform_toolchain.select_darwin_toolchain(env)
    assert toolchain.tools["clang"] == developer / "usr" / "bin" / "clang"
    assert spawns.count == 6
    _fresh_process(monkeypatch)
    assert platform_toolchain.select_darwin_toolchain(env).tools == toolchain.tools
    assert spawns.count == 6
    # A new SDK beside the selected one, and a new tool directory, are inputs.
    (developer / "SDKs" / "MacOSX.sdk").symlink_to(sdk)
    _fresh_process(monkeypatch)
    platform_toolchain.select_darwin_toolchain(env)
    assert spawns.count == 7
    (developer / "Toolchains" / "Swift.xctoolchain" / "usr" / "bin").mkdir(parents=True)
    _fresh_process(monkeypatch)
    platform_toolchain.select_darwin_toolchain(env)
    assert spawns.count == 12
    env["TOOLCHAINS"] = "swift"
    _fresh_process(monkeypatch)
    platform_toolchain.select_darwin_toolchain(env)
    assert spawns.count == 18


# --- the C driver's linker ---------------------------------------------------------


def test_link_driver_linker_is_asked_once_per_driver_and_search_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    driver = write_mock_executable(tmp_path / "clang", b"MZ clang")
    linker = write_mock_executable(tmp_path / "ld", b"MZ ld")
    spawns = _Spawns(lambda command, kwargs: ("ld\n", ""))
    monkeypatch.setattr(link_tools.process_guard, "run_completed_command", spawns)
    monkeypatch.setattr(
        link_tools, "DARWIN_DEVELOPER_SELECTION_LINK", tmp_path / "no-link"
    )
    monkeypatch.setenv("PATH", os.fspath(tmp_path))
    monkeypatch.delenv("COMPILER_PATH", raising=False)
    assert link_tools._system_linker_from_driver(driver) == linker
    assert spawns.count == 1
    assert spawns.commands[0] == [os.fspath(driver), "-print-prog-name=ld"]
    _fresh_process(monkeypatch)
    assert link_tools._system_linker_from_driver(driver) == linker
    assert spawns.count == 1
    monkeypatch.setenv("COMPILER_PATH", os.fspath(tmp_path / "elsewhere"))
    _fresh_process(monkeypatch)
    assert link_tools._system_linker_from_driver(driver) == linker
    assert spawns.count == 2
    linker.unlink()
    spawns.answer = lambda command, kwargs: ("", "")
    _fresh_process(monkeypatch)
    assert link_tools._system_linker_from_driver(driver) is None
    assert spawns.count == 3


# --- a real toolchain --------------------------------------------------------------


@pytest.mark.slow
def test_a_warm_toolchain_resolves_a_cargo_plan_without_probe_processes(
    tmp_path: Path, isolated_molt_cache: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("cargo") is None or shutil.which("rustc") is None:
        pytest.skip("real Cargo/Rust compiler capability is unavailable")
    project = tmp_path / "crate"
    (project / "src").mkdir(parents=True)
    (project / "Cargo.toml").write_text(
        f'[package]\nname = "probe"\nversion = "0.1.0"\nedition = "{RUST_EDITION}"\n',
        encoding="utf-8",
    )
    (project / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
    env = {
        name: os.environ[name]
        for name in ("PATH", "HOME", "RUSTUP_HOME", "CARGO_HOME", "USERPROFILE")
        if name in os.environ
    }
    env["CARGO_TARGET_DIR"] = os.fspath(tmp_path / "target")
    env["MOLT_EXT_ROOT"] = os.fspath(tmp_path)
    real = process_guard.run_completed_command
    probes: list[list[str]] = []

    def counted(cmd, **kwargs):  # type: ignore[no-untyped-def]
        probes.append([str(part) for part in cmd])
        return real(cmd, **kwargs)

    # Every probe class spawns through this one module attribute.
    monkeypatch.setattr(process_guard, "run_completed_command", counted)

    def resolve() -> plans.RuntimeCargoPlan:
        return plans.resolve_runtime_cargo_plan(
            project, env=env, cargo_command=["cargo", "build"], requested_target=None
        )

    first = resolve()
    cold = len(probes)
    assert cold > 0
    second = resolve()
    assert second.toolchain_identity() == first.toolchain_identity()
    _fresh_process(monkeypatch)
    probes.clear()
    third = resolve()
    assert third.toolchain_identity() == first.toolchain_identity()
    assert probes == []


@pytest.mark.parametrize("legacy_version", [1, 2])
@pytest.mark.parametrize("renumbered", [False, True])
def test_new_probe_store_starts_cold_without_reading_legacy_answers(
    tmp_path, renumbered, legacy_version
):
    import hashlib
    import time
    from molt.toolchain_identity import stable_regular_file_identity

    tool = tmp_path / "tool"
    tool.write_bytes(b"a tool generation")
    identity = stable_regular_file_identity(tool, label="migration tool")
    key = facts.ProbeKey(
        "migration",
        (facts.ProbeTool.from_identity(tool, identity),),
        ("--version",),
        None,
        (),
        (facts.file_input(tool),),
    )
    legacy_key = key.as_json()
    metadata = tool.stat()
    old_prefix = (
        [metadata.st_dev + int(renumbered), metadata.st_ino]
        if legacy_version == 1
        else ["apple-uuid:" + "a" * 32, f"{metadata.st_ino:032x}"]
    )
    for token in (legacy_key["tools"][0]["token"], legacy_key["inputs"][0][1]):
        token[:1] = old_prefix
    digest = hashlib.sha256(
        json.dumps(legacy_key, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    legacy = tmp_path / "cache" / "toolchain-facts" / f"v{legacy_version}.json"
    legacy.parent.mkdir(parents=True)
    legacy.write_text(
        json.dumps(
            {
                "schema": f"molt.toolchain-facts.v{legacy_version}",
                "facts": {
                    digest: {
                        "key": legacy_key,
                        "stdout": "old answer",
                        "stderr": "",
                        "recorded_ns": time.time_ns(),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    before = legacy.read_bytes()
    current = facts.ToolchainFactStore(tmp_path / "cache", max_age_ns=DAY_NS)
    answer = current.lookup(key)
    assert answer is None
    assert current.path == legacy.with_name("v3.json")
    assert legacy.read_bytes() == before
    assert not current.path.exists()


def test_probe_cache_tokens_ignore_device_without_querying_volume(
    tmp_path, monkeypatch
):
    from molt import durable_file_identity
    from tests.process_guard_common import install_module_view

    path = tmp_path / "input"
    path.write_bytes(b"probe input")

    def forbidden(*args, **kwargs):
        pytest.fail("a class-B probe key queried durable volume identity")

    monkeypatch.setattr(durable_file_identity, "file_identity", forbidden)
    if hasattr(facts, "file_identity"):
        monkeypatch.setattr(facts, "file_identity", forbidden)
    original = os.lstat
    before = facts.file_input(path)
    assert before[1] is not None
    metadata = path.stat()
    assert before[1].payload() == [
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    ]

    def renumbered(name):
        record = original(name)
        fields = list(record)
        fields[stat.ST_DEV] += 6
        return os.stat_result(
            fields,
            {
                name: getattr(record, name)
                for name in ("st_atime_ns", "st_mtime_ns", "st_ctime_ns")
            },
        )

    install_module_view(monkeypatch, "os", os, facts, lstat=renumbered)
    assert facts.file_input(path) == before
    assert _tool("rustc").as_json()["token"] == [
        2,
        stat.S_IFREG | 0o755,
        len(b"MZ tool"),
        3,
        4,
    ]

from __future__ import annotations

import os
import sys
from pathlib import Path
from collections.abc import Iterator
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from tests.runtime_build_identity_helper import RuntimeFixtureRoot


ROOT = Path(__file__).resolve().parents[1]
MOLT_STDLIB_ROOT = str(ROOT / "src" / "molt" / "stdlib")
_PYTEST_SENTINEL_ATTR = "_molt_repo_process_sentinel"


@pytest.fixture(autouse=True)
def _restore_process_environment() -> Iterator[None]:
    """Every test ends with the process environment it started with.

    Product code exports some selections into ``os.environ`` for the child
    processes it launches (for example ``MOLT_BACKEND``); a test that calls it
    in process would otherwise leak them into every later test on the same
    worker. Session-scoped fixtures run before this one, so their settings
    persist as intended.
    """
    snapshot = dict(os.environ)
    yield
    if os.environ != snapshot:
        os.environ.clear()
        os.environ.update(snapshot)


# Process-global names the intrinsic loader (src/_intrinsics.py) reads.
_INTRINSIC_BUILTINS = (
    "_molt_intrinsics",
    "_molt_intrinsic_lookup",
    "_molt_intrinsics_strict",
    "_molt_runtime",
)


@pytest.fixture(autouse=True)
def _restore_intrinsic_registry() -> Iterator[None]:
    """Every test ends with the intrinsic registry it started with.

    Stub-surface tests install fake intrinsic tables on ``builtins`` to run a
    stdlib module on the host interpreter, and some add entries in place. A
    leaked table answers ``molt_capabilities_has`` and friends for every later
    test on the same worker, so the restore covers both the binding and the
    dictionary contents.
    """
    import builtins

    missing = object()
    saved = {name: getattr(builtins, name, missing) for name in _INTRINSIC_BUILTINS}
    contents = {
        name: dict(value) for name, value in saved.items() if isinstance(value, dict)
    }
    yield
    for name, value in saved.items():
        if value is missing:
            if hasattr(builtins, name):
                delattr(builtins, name)
            continue
        setattr(builtins, name, value)
        if name in contents and value != contents[name]:
            value.clear()
            value.update(contents[name])


# Guard caps a CI job plan or an outer guard exports to its children.
AMBIENT_GUARD_CAP_KEYS = (
    "MOLT_MAX_PROCESS_RSS_GB",
    "MOLT_MAX_TOTAL_RSS_GB",
    "MOLT_MAX_GLOBAL_RSS_GB",
)


@pytest.fixture
def no_ambient_guard_caps(monkeypatch: pytest.MonkeyPatch) -> None:
    """Resolve guard limits as if no CI plan or outer guard had set caps.

    CI exports plan-derived caps for every guarded child, so a test of how
    limits resolve must not inherit them; it sets the caps it means to test.
    """
    for key in AMBIENT_GUARD_CAP_KEYS:
        monkeypatch.delenv(key, raising=False)


# What a hosted job adds to a developer host: the custody contract, and the
# Molt roots and session that the guarded executor derives from it. Tool caches
# (UV_*, TMPDIR, PYTHONPYCACHEPREFIX) stay, so child `uv run` calls keep their
# environment and write nothing into the checkout.
DEVELOPER_HOST_CLEARED_KEYS = (
    "MOLT_CI_EPHEMERAL_CUSTODY_ROOT",
    "MOLT_EXT_ROOT",
    "MOLT_TARGET_ROOT",
    "MOLT_CACHE",
    "MOLT_DIFF_ROOT",
    "MOLT_DIFF_TMPDIR",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "CARGO_TARGET_DIR",
    "MOLT_SESSION_ID",
    "MOLT_SESSION_ID_GENERATED",
)


@pytest.fixture
def developer_host_context(monkeypatch: pytest.MonkeyPatch) -> None:
    """Resolve paths as a developer host with no ambient run context does.

    A hosted job exports the custody root for the whole job, and the proof
    plan's guarded executor exports the roots and session it derives from it.
    A test that builds a synthetic project, patches ``subprocess`` or asserts
    default roots would test that CI context instead.
    """
    from molt.dx import GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV

    assert GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV in DEVELOPER_HOST_CLEARED_KEYS
    for key in DEVELOPER_HOST_CLEARED_KEYS:
        monkeypatch.delenv(key, raising=False)


@pytest.fixture
def session_sentinel_paused(request: pytest.FixtureRequest) -> Iterator[None]:
    """Pause the serial session's repo sentinel while a test fakes process data.

    The sentinel's thread reads the same module functions these tests patch, so
    it could see one test's fake groups and act on them during the next test.
    """
    sentinel = getattr(request.config, _PYTEST_SENTINEL_ATTR, None)
    if sentinel is None:
        yield
        return
    with sentinel.paused():
        yield


@pytest.fixture
def isolated_molt_cache(tmp_path_factory, monkeypatch) -> Path:
    """Opt-in disposable cache outside a test's compiler/project source tree."""
    cache = tmp_path_factory.mktemp("molt-cache")
    monkeypatch.setenv("MOLT_CACHE", str(cache))
    return cache


@pytest.fixture
def admitted_build_capacity(monkeypatch: pytest.MonkeyPatch) -> None:
    """Admit build-capacity probes for tests whose Cargo runs are fakes.

    Real admission samples host free space against the 25 GiB default. A test
    that writes no real build output must not pass or fail with the host disk;
    tests of admission itself patch the probe explicitly after this fixture.
    """
    from molt import disk_capacity

    monkeypatch.setattr(
        disk_capacity,
        "_default_measure_free_bytes",
        lambda _path: disk_capacity.DEFAULT_MINIMUM_HEADROOM_BYTES + 1,
    )


@pytest.fixture
def generous_host_memory(monkeypatch: pytest.MonkeyPatch) -> None:
    """Pin the adaptive memory budget to a large host.

    The guard clamps even explicit limits to the host's adaptive global budget,
    so a test of how explicit limits resolve must not pass or fail with the
    runner's memory (a 7 GB CI runner clamps a 3 GB cap to about 2 GB).
    """
    from tools import memory_guard

    def budget(prefix=None, environ=None, *, accounted_rss_kb=0):
        return memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=48,
            max_total_rss_gb=56,
            max_global_rss_gb=60,
            reserve_gb=4,
            physical_gb=64,
            available_gb=60,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    monkeypatch.setattr(memory_guard, "adaptive_memory_budget", budget)


@pytest.fixture
def runtime_fixture_root(tmp_path: Path) -> RuntimeFixtureRoot:
    """Separate writable synthetic runtime tools from compiler source custody."""
    from tests.runtime_build_identity_helper import RuntimeFixtureRoot

    return RuntimeFixtureRoot(tmp_path)


def _remove_molt_stdlib_top_level_root() -> None:
    """Keep host pytest imports on CPython's stdlib.

    Surface tests may load Molt stdlib files directly, but `src/molt/stdlib`
    must not remain as a top-level import root during collection. If it does,
    host imports such as `ctypes`, `fractions`, `statistics`, and `tarfile`
    resolve to Molt intrinsic-gated wrappers and fail before the runtime exists.
    """

    while MOLT_STDLIB_ROOT in sys.path:
        sys.path.remove(MOLT_STDLIB_ROOT)


def _ensure_src_on_path() -> None:
    for subdir in ("src", "tools"):
        p = str(ROOT / subdir)
        if p not in sys.path:
            sys.path.insert(0, p)
    _remove_molt_stdlib_top_level_root()


def _ensure_pytest_process_scope() -> None:
    # Under pytest-xdist each worker is a separate process that inherits the
    # master's environment. A plain ``setdefault`` would leave every worker
    # sharing the master's ``MOLT_SESSION_ID``, collapsing their backend
    # daemons, build state, and compile cache onto a single session — which
    # serialises (and races) compilation and makes parallel runs fail. Give
    # each xdist worker a distinct, stable session keyed on its worker id
    # (``gw0``/``gw1``/…); fall back to the pid for serial (non-xdist) runs.
    worker = os.environ.get("PYTEST_XDIST_WORKER")
    if worker:
        os.environ["MOLT_SESSION_ID"] = f"pytest-xdist-{worker}"
        os.environ["MOLT_SESSION_ID_GENERATED"] = "1"
    elif "MOLT_SESSION_ID" not in os.environ:
        os.environ["MOLT_SESSION_ID"] = f"pytest-{os.getpid()}"
        os.environ["MOLT_SESSION_ID_GENERATED"] = "1"


def _assert_pytest_memory_guard_active() -> None:
    from molt import pytest_memory_guard_bootstrap

    pytest_args = pytest_memory_guard_bootstrap.python_pytest_invocation_args()
    try:
        pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
            os.environ,
            args=pytest_args if pytest_args is not None else tuple(sys.argv[1:]),
        )
    except SystemExit as exc:
        raise pytest.UsageError(str(exc)) from exc
    if pytest_memory_guard_bootstrap.outer_memory_guard_active():
        return
    raise RuntimeError(
        "Molt pytest custody requires a live ancestor tools/memory_guard.py "
        "process before collection; run pytest from the repo root so "
        "sitecustomize.py or the configured pytest plugins can re-exec under "
        "the memory guard."
    )


def pytest_configure() -> None:
    _ensure_src_on_path()
    _assert_pytest_memory_guard_active()
    _ensure_pytest_process_scope()


def _is_xdist_run(session) -> bool:  # type: ignore[no-untyped-def]
    """True when this pytest invocation runs under pytest-xdist (parallel).

    Detected in workers via ``PYTEST_XDIST_WORKER`` and in the controller via
    the resolved ``-n`` value (``numprocesses``).
    """
    if os.environ.get("PYTEST_XDIST_WORKER"):
        return True
    try:
        return bool(session.config.option.numprocesses)
    except AttributeError:
        return False


def pytest_sessionstart(session) -> None:  # type: ignore[no-untyped-def]
    _ensure_src_on_path()
    _ensure_pytest_process_scope()
    # Automatic repo sentinels now scope violation/drain kills to the current
    # process tree, but xdist still has many independent worker controllers and
    # its own channel teardown. Keep the session sentinel serial-only: each xdist
    # compliance build is already bounded by the per-build memory guard in
    # tests/compliance/process_guard.run_compliance_process, and the worker count
    # caps aggregate concurrency. Serial runs keep the full sentinel.
    if _is_xdist_run(session):
        return
    from tools import harness_memory_guard
    from molt.pytest_memory_guard_bootstrap import outer_guard_summary_dir

    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=ROOT,
        artifact_root=outer_guard_summary_dir(),
        label=f"pytest-{os.getpid()}",
        limits=harness_memory_guard.limits_from_env("MOLT_PYTEST"),
        drain_on_exit=True,
    )
    setattr(session.config, _PYTEST_SENTINEL_ATTR, sentinel)
    sentinel.__enter__()


def pytest_sessionfinish(session, exitstatus) -> None:  # type: ignore[no-untyped-def]
    sentinel = getattr(session.config, _PYTEST_SENTINEL_ATTR, None)
    if sentinel is not None:
        sentinel.__exit__(None, None, None)


def pytest_collect_file() -> None:
    _remove_molt_stdlib_top_level_root()


@pytest.fixture
def cargo_output_implementation_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    """Model source and Cargo outputs as siblings even with an in-checkout basetemp.

    CI places pytest's basetemp under the checkout's tmp/, so a declared
    "external" output root would otherwise overlap the implementation source
    and trip the production supervisor-store overlap check. Lives in the root
    conftest because directory conftest discovery under the non-package
    tests/tools directory is unreliable when collected with package modules.
    """
    from tools.proof_queue_pkg import cargo_output_layout

    source = tmp_path / "implementation-source"
    source.mkdir()
    monkeypatch.setattr(
        cargo_output_layout, "implementation_source_root", lambda: source
    )
    return source

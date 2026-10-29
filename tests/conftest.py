from __future__ import annotations

import hashlib
import importlib.util
import os
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path
from collections.abc import Callable, Iterator, Mapping
from typing import TYPE_CHECKING

import pytest

from tools.import_file import bind_repository_imports, require_repository_identity

if TYPE_CHECKING:
    from molt.llvm_toolchain import WasiSdkInstallation
    from tests.integration_driver_fixture import GitFixture
    from tests.runtime_build_identity_helper import RuntimeFixtureRoot


ROOT = Path(__file__).resolve().parents[1]
MOLT_STDLIB_ROOT = str(ROOT / "src" / "molt" / "stdlib")
_PYTEST_SENTINEL_ATTR = "_molt_repo_process_sentinel"
_PYTEST_DAEMON_SCOPE_ATTR = "_molt_daemon_suite_scope"
_CHECKOUT_TARGET_SNAPSHOT_ATTR = "_molt_checkout_target_snapshot"
# The CLI's default Cargo target for a project; the compiler's own checkout
# must never get one from a test session (HF-114).
CHECKOUT_TARGET = ROOT / "target"


@pytest.fixture
def short_cargo_cache() -> Iterator[Path]:
    """Owned temporary cache whose native startup socket fits on macOS."""
    import tempfile
    from molt.dx import artifact_root

    parent = artifact_root(ROOT) / "tmp"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cenv-", dir=parent) as raw:
        yield Path(raw) / "cache"


@pytest.fixture(autouse=True)
def _storage_write_authority(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[None]:
    """New Molt Python writers must use a declared storage class/volume.

    Temporary fixture roots are explicit scratch outputs. The check observes
    actual mutation events, so a direct Path write cannot bypass the typed DX
    API unnoticed when the producer's test runs.
    """
    from molt.dx import StorageWriteClaim, storage_write_check

    with storage_write_check(
        ROOT,
        os.environ,
        outputs=(StorageWriteClaim("temporary", tmp_path_factory.getbasetemp()),),
    ):
        yield


@pytest.fixture(autouse=True)
def _restore_process_environment() -> Iterator[None]:
    """Every test ends with the process environment it started with.

    Product code passes its selections to child processes in their own
    environment mappings and never writes ``os.environ`` (HF-60), but a test
    may still set variables directly; this keeps them from leaking into every
    later test on the same worker. Session-scoped fixtures run before this
    one, so their settings persist as intended.

    Each test also starts with disk scratch: the operator's
    ``MOLT_SCRATCH_STORAGE`` governs the guarded session, whose scratch the
    memory guard allocated before pytest started. Tests assert projected
    scratch paths; a test of memory storage sets the mode itself.
    """
    snapshot = dict(os.environ)
    os.environ.pop("MOLT_SCRATCH_STORAGE", None)
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
def _isolated_product_stores(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[None]:
    """In-process builds keep their products out of the host's product store.

    The store keys entries by input identity (design 56, section 2.8), so a
    test that fakes a product under a fixture identity would leave it for the
    next run, and a test that expects a store miss would find it. Each test
    re-roots every store it resolves below a directory of its own; distinct
    stores stay distinct. Builds a test runs as a child process keep the host
    store, where real products are worth reusing.

    It patches through a private ``MonkeyPatch``: requesting the test's own
    ``monkeypatch`` here would set it up before ``_restore_intrinsic_registry``
    and so tear it down after that fixture, which then deletes the intrinsic
    builtins a test patched before ``monkeypatch`` can restore them.
    """
    from molt.cli import product_store

    resolve = product_store.product_store_dir
    root = tmp_path_factory.getbasetemp() / "product-stores" / uuid.uuid4().hex

    def isolated(project_root: Path, env: Mapping[str, str]) -> Path:
        store = os.fspath(resolve(project_root, env))
        return root / hashlib.sha256(store.encode()).hexdigest()[:16]

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(product_store, "product_store_dir", isolated)
        yield


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

    CI exports plan-derived caps for every guarded child, and a guard or the
    proof executor exports a memory slice that bounds nested budgets, so a
    test of how limits resolve must not inherit them; it sets the caps it
    means to test.
    """
    from molt import host_capacity

    for key in (*AMBIENT_GUARD_CAP_KEYS, host_capacity.MEMORY_SLICE_ENV):
        monkeypatch.delenv(key, raising=False)


# The session's environment once its run context is in place (pytest_configure).
_SESSION_ENVIRONMENT: dict[str, str] = {}


def _run_context_env_keys() -> tuple[str, ...]:
    """The ambient run context: hosted custody, request knobs, roots, session.

    An operator's explicit external artifact roots are run context too: with
    them, `prefer_external_artifacts` in pyproject.toml moves default roots off
    the checkout family's custody root.
    """
    from molt.dx import (
        DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS,
        EXTERNAL_ARTIFACT_ROOTS_ENV,
        GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV,
        MOLT_ROOT_ENV_KEYS,
    )

    return (
        GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV,
        EXTERNAL_ARTIFACT_ROOTS_ENV,
        *MOLT_ROOT_ENV_KEYS,
        *DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS,
        "MOLT_SESSION_ID",
        "MOLT_SESSION_ID_GENERATED",
    )


@pytest.fixture
def developer_host_context(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path_factory: pytest.TempPathFactory,
) -> None:
    """Resolve paths as a developer host with no ambient run context does.

    A hosted job exports the custody root for the whole job, a workflow may
    request external artifacts for every step (``MOLT_PREFER_EXTERNAL_ARTIFACTS``),
    and every test session enters the Molt roots and session of its run context
    (``molt.dx.MOLT_ROOT_ENV_KEYS``). A test that builds a synthetic project,
    patches ``subprocess`` or asserts default roots would test that context
    instead. Tool caches (UV_*, TMPDIR, PYTHONPYCACHEPREFIX) stay, so child
    `uv run` calls keep their environment and write nothing into the checkout.
    A child that runs repository tooling on the real checkout needs the
    context back: see `checkout_run_context`.

    Without ``MOLT_CACHE`` an unbound developer host's Molt cache falls back to the
    user cache base (``molt.default_paths``), so the test gets its own base
    (``XDG_CACHE_HOME``, or ``LOCALAPPDATA`` on Windows) instead of writing
    into the developer's real home cache (HF-562). ``HOME`` stays, because
    Cargo, rustup and uv find their installations through it.
    """
    for key in _run_context_env_keys():
        monkeypatch.delenv(key, raising=False)
    cache_base = "LOCALAPPDATA" if os.name == "nt" else "XDG_CACHE_HOME"
    monkeypatch.setenv(cache_base, str(tmp_path_factory.mktemp("user-cache")))


@pytest.fixture
def unbound_checkout_family(monkeypatch: pytest.MonkeyPatch) -> None:
    """Resolve this checkout's family as unbound inside the test process.

    A developer host may bind the family's artifact root
    (`.git/molt/artifact-root.json`); hosted CI never does. A test whose subject
    is unbound selection for the real checkout uses this. Only this process
    stops seeing the binding: a child process still reads it from disk, so a
    test whose children resolve roots runs them on an unbound scratch clone.
    """
    from molt import custody_layout, dx

    real_state = custody_layout.checkout_family_state_root(ROOT)
    if real_state is None:
        return
    original = dx.artifact_root_binding_path

    def binding_path(repo_root: Path, *, common_dir: Path | None = None) -> Path | None:
        path = original(repo_root, common_dir=common_dir)
        return None if path is not None and path.parent == real_state else path

    monkeypatch.setattr(dx, "artifact_root_binding_path", binding_path)


@pytest.fixture
def checkout_run_context() -> dict[str, str]:
    """The session's run context, for a child that runs on the real checkout.

    `developer_host_context` clears it. A child that runs repository tooling
    on the real checkout (``tools/dev.py``, ``python -m molt.cli``) must get it
    back: without it a hosted CI clone is its own artifact root, and the child
    creates ``target/``, ``.molt_cache/``, ``.uv-cache/`` and a uv environment
    inside the checkout.
    """
    from molt.dx import GITHUB_ACTIONS_CUSTODY_ENV_KEYS

    # Restore the actual session's complete claims, not only its custody root.
    # Do not add these to developer_host_context's clearing set: RUNNER_TEMP
    # remains a host scratch fact even while a test isolates its project roots.
    keys = (*_run_context_env_keys(), *GITHUB_ACTIONS_CUSTODY_ENV_KEYS)
    return {key: value for key, value in _SESSION_ENVIRONMENT.items() if key in keys}


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


@pytest.fixture(autouse=True)
def _isolate_host_toolchain_facts(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[None]:
    """Fake probe answers never enter the developer's shared host store.

    Redirect the defining selector, so consumers that imported it by value
    share this namespace too. Allocate it only when a test asks for facts;
    unrelated tests pay no directory creation. The memo has the same test
    lifetime. Explicit ToolchainFactStore/toolchain_fact_store roots retain
    their on-disk sharing contract; child integrations use isolated_molt_cache
    to pass their chosen private namespace through the process environment.
    """
    from threading import Lock

    from molt import toolchain_facts, toolchain_identity

    digest_caches = toolchain_identity._FILE_DIGEST_CACHES
    cache: Path | None = None
    allocation_lock = Lock()

    def private_cache() -> Path:
        nonlocal cache
        if cache is None:
            with allocation_lock:
                if cache is None:
                    cache = tmp_path_factory.mktemp("toolchain-facts")
        return cache

    # A test may undo its own monkeypatches before a real probe. Its undo
    # must not release the fixture's namespace or process-memo custody.
    with pytest.MonkeyPatch.context() as isolation:
        isolation.setattr(toolchain_facts, "_default_molt_cache", private_cache)
        isolation.setattr(toolchain_facts, "_STORES", {})
        try:
            yield
        finally:
            # Probe-tool digests use the same namespace. Retire only this test's
            # private memo entry; unrelated real build-cache entries stay warm.
            if cache is not None:
                digest_caches.pop(toolchain_facts.file_digest_cache_path(cache), None)


@pytest.fixture
def isolated_molt_cache(tmp_path_factory, monkeypatch) -> Path:
    """Disposable cache outside the test's compiler/project source tree.

    Synthetic fingerprints and cache-hit/no-recomputation oracles opt in here.
    The default shared cache can legitimately be busy or evicted by other runs;
    a test of cross-process sharing explicitly shares this private root instead.
    """
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


@pytest.fixture(scope="session", autouse=True)
def _isolate_lima_home(tmp_path_factory: pytest.TempPathFactory) -> Iterator[None]:
    """Tests never reach the developer's Lima VMs.

    Lane retirement makes a running VM that mounts host paths release the
    host files it pins (``tools/linux_vm.py``); a test that retires a
    checkout must not act on a real guest. An empty Lima home has no live hostagent, so that release
    reads one directory and spawns nothing. Child processes inherit it.
    """
    previous = os.environ.get("LIMA_HOME")
    os.environ["LIMA_HOME"] = str(tmp_path_factory.mktemp("lima-home"))
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop("LIMA_HOME", None)
        else:
            os.environ["LIMA_HOME"] = previous


@pytest.fixture(scope="session", autouse=True)
def _native_fixture_donor(tmp_path_factory: pytest.TempPathFactory) -> Iterator[None]:
    """Bind a lazy donor without loading runtime construction in unrelated tests."""
    from tests.executable_test_support import bind_native_donor_factory

    bind_native_donor_factory(
        lambda: tmp_path_factory.mktemp("native-donor"), environment=os.environ
    )
    try:
        yield
    finally:
        bind_native_donor_factory(None, environment=None)


@pytest.fixture(scope="session")
def driver_seed(tmp_path_factory: pytest.TempPathFactory, worker_id: str) -> Path:
    """This working tree's driver files, copied once per session; clone from it.

    Fixture repositories that land through the real driver
    (tests/integration_driver_fixture.py) take their ``tools`` and ``src/molt``
    from here: one copy across volumes per session, then block-sharing clones.
    """
    from tests.integration_driver_fixture import copy_driver_trees, shared_session_tree

    return shared_session_tree(
        tmp_path_factory, worker_id, "driver-seed", copy_driver_trees
    )


@pytest.fixture(scope="session")
def driver_history_seed(
    tmp_path_factory: pytest.TempPathFactory, worker_id: str, driver_seed: Path
) -> GitFixture:
    """One immutable committed driver, shared across integration test families."""
    from tests.integration_driver_fixture import (
        GitFixture,
        build_driver_history,
        shared_session_tree,
    )

    root = shared_session_tree(
        tmp_path_factory,
        worker_id,
        "driver-history",
        lambda root: build_driver_history(root, driver_seed),
    )
    return GitFixture(root / "origin.git", root / "work")


@pytest.fixture(scope="session")
def native_fixture_donor(_native_fixture_donor: None) -> Path:
    """Build before a requesting test runs: its body may forbid child launches."""
    from tests.executable_test_support import native_executable_donor

    return native_executable_donor()


@pytest.fixture
def runtime_fixture_root(
    tmp_path: Path, native_fixture_donor: Path
) -> RuntimeFixtureRoot:
    """Separate writable synthetic runtime tools from compiler source custody."""
    from tests.runtime_build_identity_helper import RuntimeFixtureRoot

    return RuntimeFixtureRoot(tmp_path)


@pytest.fixture(scope="module")
def readonly_wasi_sdk(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[Callable[[], WasiSdkInstallation]]:
    """Lazily own one immutable SDK generation for this module's readers.

    Mutable SDK and relocation controls must provision a private generation.
    Runtime sources, resources, plans and outputs remain function-owned.
    """
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )

    from molt.cli.compiler_metadata import _compiler_root
    from molt.llvm_toolchain import load_wasi_sdk_installation

    prefix = None
    modes = []

    def provision():
        nonlocal prefix
        if prefix is None:
            owner = RuntimeFixtureRoot(tmp_path_factory.mktemp("readonly-wasi-sdk"))
            created = provisioned_wasi_sdk_fixture(owner)
            paths = sorted(
                (created.prefix, *created.prefix.rglob("*")),
                key=lambda path: len(path.parts),
            )
            modes.extend((path, path.stat().st_mode) for path in paths)
            for path, mode in reversed(modes):
                path.chmod(mode & ~0o222)
            prefix = created.prefix
        # Share the immutable files, never a caller-mutable decoded fact map.
        return load_wasi_sdk_installation(_compiler_root(), prefix, verify_tree=False)

    try:
        yield provision
    finally:
        # Restore only this fixture's owned modes so pytest can remove it.
        for path, mode in modes:
            path.chmod(mode)


@pytest.fixture
def readonly_file_source(
    tmp_path: Path,
) -> Iterator[tuple[Path, Callable[[], int]]]:
    """Observe a shared readonly source through a handle, not cached link metadata."""
    source = tmp_path / "external-readonly-source"
    source.write_bytes(b"external source must survive cleanup")
    source.chmod(0o444)
    try:
        if os.name == "nt":
            from molt import file_hashing

            api = file_hashing._windows_file_api()
            assert api is not None
            ctypes, kernel32, file_basic_info = api
            handle = kernel32.CreateFileW(
                str(source), 0x0080, 0x1 | 0x2 | 0x4, None, 3, 0x00200000, None
            )
            assert handle not in (None, ctypes.c_void_p(-1).value)

            def attributes() -> int:
                info = file_basic_info()
                if not kernel32.GetFileInformationByHandleEx(
                    handle, 0, ctypes.byref(info), ctypes.sizeof(info)
                ):
                    raise ctypes.WinError(ctypes.get_last_error())
                return int(info.FileAttributes)

            try:
                assert attributes() & 0x1  # FILE_ATTRIBUTE_READONLY
                yield source, attributes
            finally:
                assert kernel32.CloseHandle(handle)
        else:
            import stat

            with source.open("rb") as handle:

                def mode() -> int:
                    return stat.S_IMODE(os.fstat(handle.fileno()).st_mode)

                assert not mode() & 0o222
                yield source, mode
    finally:
        # The fixture owns this source. Restoration is teardown only, after
        # the consumer's preservation assertions and all source handles close.
        source.chmod(0o600)


def _remove_molt_stdlib_top_level_root() -> bool:
    """Keep host pytest imports on CPython's stdlib; True if a root was found.

    Surface tests may load Molt stdlib files directly, but `src/molt/stdlib`
    must never be a top-level import root of the host process. If it is,
    host imports such as `asyncio`, `concurrent`, `ctypes` and `tarfile`
    resolve to Molt intrinsic-gated wrappers and fail with "runtime inactive"
    in every later test on the worker, and in every child it spawns.
    """

    found = MOLT_STDLIB_ROOT in sys.path
    while MOLT_STDLIB_ROOT in sys.path:
        sys.path.remove(MOLT_STDLIB_ROOT)
    return found


_MOLT_STDLIB_ROOT_LEAK = (
    "{owner} left {root} on sys.path; Molt's stdlib then shadows CPython's "
    "for the rest of the process. Load Molt stdlib files by path "
    "(tests/stdlib_intrinsic_registry.py, tests/helpers/tinygrad_stdlib_loader.py) "
    "or in a child interpreter."
)


@pytest.fixture(autouse=True)
def _reject_molt_stdlib_import_root(request: pytest.FixtureRequest) -> Iterator[None]:
    """Fail the test that puts Molt's stdlib on the host import path."""
    yield
    if _remove_molt_stdlib_top_level_root():
        pytest.fail(
            _MOLT_STDLIB_ROOT_LEAK.format(
                owner=request.node.nodeid, root=MOLT_STDLIB_ROOT
            )
        )


_spec_from_file_location = importlib.util.spec_from_file_location


def _one_repository_identity(name, location=None, *args, **kwargs):  # type: ignore[no-untyped-def]
    """``spec_from_file_location`` that refuses a second repository identity.

    A file under ``tools/`` is the module ``tools.<path>`` (HF-221) and a Molt
    host source is ``molt.<path>`` (HF-223). Loading either by path under
    another name builds a second copy of its state and types, so the test or
    tool that does so fails at the load. Normal imports use the import
    system's own binding of this function and never come here.
    """
    if location is not None:
        require_repository_identity(name, location)
    return _spec_from_file_location(name, location, *args, **kwargs)


importlib.util.spec_from_file_location = _one_repository_identity


@dataclass(frozen=True)
class GuardCustodyRoots:
    """Where guards keep custody in this session, and where they must not."""

    # The state root of every guard a test starts.
    state_root: Path
    # The roots this session's own outer guard uses (the host's by default).
    host_state_root: Path
    host_scratch_root: Path


@pytest.fixture(scope="session", autouse=True)
def test_guard_custody_roots(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[GuardCustodyRoots]:
    """Guards that tests start keep their custody inside this session.

    This pytest run's own outer guard is a real guard of the run: its marker
    and scratch stay in the host's guard state root, where disk reclamation
    sees them, and they protect the checkout while the run lasts. Every guard
    a test starts, in process or as a child, inherits the state root set
    here. Its marker, scratch generation and lease target then live under
    this session's basetemp, which the outer guard reclaims with the run, so
    no test writes the host's guard records. A test that needs another root
    sets its own.
    """
    from molt import pytest_memory_guard_bootstrap, temporary_artifacts
    from molt.memory_guard_paths import STATE_ROOT_ENV, memory_guard_state_root

    # Admit this process's current-test record under the outer guard's root
    # first: the bootstrap keeps a path it admitted.
    pytest_memory_guard_bootstrap.install_pytest_current_test_file_env()
    roots = GuardCustodyRoots(
        state_root=tmp_path_factory.mktemp("gc", numbered=False) / "memory_guard",
        host_state_root=memory_guard_state_root(ROOT, os.environ),
        host_scratch_root=temporary_artifacts.scratch_root(ROOT, os.environ),
    )
    previous = os.environ.get(STATE_ROOT_ENV)
    os.environ[STATE_ROOT_ENV] = str(roots.state_root)
    yield roots
    if previous is None:
        os.environ.pop(STATE_ROOT_ENV, None)
    else:
        os.environ[STATE_ROOT_ENV] = previous


def _bind_repository_import_path() -> None:
    # The one binding authority; no directory inside tools/ belongs on the
    # path (tools/profile.py would shadow the stdlib `profile`).
    bind_repository_imports(__file__)
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


def _assert_pytest_memory_guard_active(config: pytest.Config) -> None:
    from molt import pytest_memory_guard_bootstrap

    # pytest's own invocation arguments: the command line, or the list an
    # in-process pytest.main() received, which sys.argv does not show.
    try:
        pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
            os.environ,
            args=tuple(str(value) for value in config.invocation_params.args),
            invocation_dir=Path(config.invocation_params.dir),
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


def _keep_hypothesis_storage_in_scratch() -> None:
    """Keep Hypothesis's home directory in scratch, never the checkout.

    Hypothesis defaults to ``.hypothesis/`` under the working directory, the
    checkout, and keeps its example database and its constants and Unicode
    caches there (HF-135). The home is per checkout, so examples stay
    reproducible across this checkout's sessions.
    """
    try:
        from hypothesis.configuration import set_hypothesis_home_dir
    except ImportError:
        return
    from molt.checkout_state import checkout_scratch_state_dir

    set_hypothesis_home_dir(
        checkout_scratch_state_dir("hypothesis-home", ROOT, os.environ)
    )


def pytest_configure(config: pytest.Config) -> None:
    _bind_repository_import_path()
    _assert_pytest_memory_guard_active(config)
    _ensure_pytest_process_scope()
    _keep_hypothesis_storage_in_scratch()
    _SESSION_ENVIRONMENT.clear()
    _SESSION_ENVIRONMENT.update(os.environ)


def pytest_collection_modifyitems(
    config: pytest.Config, items: list[pytest.Item]
) -> None:
    """Turn each `@finding` mark into its strict expected failure.

    A finding test lives under tests/findings/ so the coverage projection
    (tools/finding_coverage.py) sees every one, names exactly one open finding,
    and takes its expected failure only from that finding.
    """
    from tests import findings

    problems: list[str] = []
    for item in items:
        marks = tuple(item.iter_markers(name=findings.MARK_NAME))
        if not marks:
            continue
        problem = findings.finding_test_problem(
            item.path,
            marks,
            explicit_xfail=any(True for _ in item.iter_markers(name="xfail")),
        )
        if problem is not None:
            problems.append(f"{item.nodeid}: {problem}")
            continue
        item.add_marker(findings.strict_expectation(marks[0].args[0]))
    if problems:
        raise pytest.UsageError("invalid finding tests:\n  " + "\n  ".join(problems))


def _is_xdist_run(session) -> bool:  # type: ignore[no-untyped-def]
    """True when this pytest invocation runs under pytest-xdist (parallel).

    Detected in workers via pytest's ``workerinput`` and in the controller via
    the resolved ``-n`` value (``numprocesses``).
    """
    if hasattr(session.config, "workerinput"):
        return True
    try:
        return bool(session.config.option.numprocesses)
    except AttributeError:
        return False


def checkout_target_entries(target: Path = CHECKOUT_TARGET) -> frozenset[str]:
    """Paths a session could create in the checkout.

    Every top-level entry of the checkout, plus the children of its own Cargo
    target and of that target's session-scoped targets (``sessions/*``),
    relative to the checkout. Reads three directory listings, so it costs
    nothing on a large checkout or target.
    """
    entries: set[str] = set()
    try:
        entries.update(os.listdir(target.parent))
    except OSError:
        pass
    for directory in (target, target / "sessions"):
        try:
            names = os.listdir(directory)
        except OSError:
            continue
        prefix = directory.relative_to(target.parent).as_posix()
        entries.update(f"{prefix}/{name}" for name in names)
    return frozenset(entries)


def checkout_target_leaks(
    before: frozenset[str],
    after: frozenset[str],
    *,
    cargo_target_in_checkout: bool,
) -> tuple[str, ...]:
    """Entries a session added to the checkout.

    A checkout outside its artifact root must gain none. A checkout that is
    its own artifact root (a plain clone) builds in its own target, but a
    pytest session id never scopes a target there, and nothing else may
    appear beside the target.
    """
    added = sorted(after - before)
    if cargo_target_in_checkout:
        return tuple(
            entry
            for entry in added
            if entry.startswith("target/sessions/pytest-")
            or not (entry == "target" or entry.startswith("target/"))
        )
    return tuple(added)


def _is_xdist_worker() -> bool:
    return bool(os.environ.get("PYTEST_XDIST_WORKER"))


def _describe_checkout_leaks(leaks: tuple[str, ...]) -> list[str]:
    """Each leaked entry with up to five of its children, which name its producer."""
    described = []
    for entry in leaks:
        path = ROOT / entry
        try:
            children = sorted(os.listdir(path))[:5] if path.is_dir() else []
        except OSError:
            children = []
        suffix = f" [{', '.join(children)}]" if children else ""
        described.append(entry + suffix)
    return described


def _cargo_target_in_checkout() -> bool:
    from molt.path_custody import host_path_is_within

    cargo_target = os.environ.get("CARGO_TARGET_DIR", "").strip()
    return bool(cargo_target) and host_path_is_within(Path(cargo_target), ROOT)


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_protocol(item, nextitem):  # type: ignore[no-untyped-def]
    """Name the test that was running when build state entered the checkout."""
    before = checkout_target_entries()
    yield
    leaks = checkout_target_leaks(
        before,
        checkout_target_entries(),
        cargo_target_in_checkout=_cargo_target_in_checkout(),
    )
    if leaks:
        item.warn(
            pytest.PytestWarning(
                "build state appeared in the checkout while this test ran "
                f"(a parallel worker may share the window): "
                f"{'; '.join(_describe_checkout_leaks(leaks))}"
            )
        )


def _report_checkout_target_leaks(session) -> None:  # type: ignore[no-untyped-def]
    before = getattr(session.config, _CHECKOUT_TARGET_SNAPSHOT_ATTR, None)
    if before is None:
        return
    leaks = checkout_target_leaks(
        before,
        checkout_target_entries(),
        cargo_target_in_checkout=_cargo_target_in_checkout(),
    )
    if not leaks:
        return
    session.exitstatus = pytest.ExitCode.TESTS_FAILED
    lines = [
        f"Build or test state appeared in the checkout during this session: {ROOT}",
        *(f"  {entry}" for entry in _describe_checkout_leaks(leaks)),
        "Tests build and keep state where a developer run does "
        "(molt.dx.RunContext.root_env, molt.dx.scratch_dir). The warnings "
        "summary names each test that was running when an entry appeared "
        "(HF-114, HF-135).",
        "Every session that shares the checkout reports the same entries: a "
        "proof-plan family runs its commands in parallel.",
    ]
    reporter = session.config.pluginmanager.get_plugin("terminalreporter")
    if reporter is None:
        print("\n".join(lines), file=sys.stderr)
        return
    reporter.ensure_newline()
    reporter.write_sep("!", "checkout build state", red=True)
    for line in lines:
        reporter.write_line(line, red=True)


@pytest.hookimpl(tryfirst=True)
def pytest_sessionstart(session) -> None:  # type: ignore[no-untyped-def]
    _bind_repository_import_path()
    _ensure_pytest_process_scope()
    if not _is_xdist_worker():
        # Before the repo sentinel, whose suite lease is the first writer of
        # build state in a session.
        setattr(
            session.config, _CHECKOUT_TARGET_SNAPSHOT_ATTR, checkout_target_entries()
        )
    # xdist's trylast hook starts workers after this hook. One controller lease
    # is inherited by every worker; individual build guards transfer shared
    # daemons to it instead of killing them while peer requests are in flight.
    # Per-build guards retain RSS admission; no additional xdist scanner runs.
    if _is_xdist_run(session):
        if not hasattr(session.config, "workerinput"):
            from molt.backend_daemon_suite_custody import daemon_suite_scope

            scope = daemon_suite_scope(project_root=ROOT, environ=os.environ)
            scope.__enter__()
            setattr(session.config, _PYTEST_DAEMON_SCOPE_ATTR, scope)
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


@pytest.hookimpl(trylast=True)
def pytest_sessionfinish(session, exitstatus) -> None:  # type: ignore[no-untyped-def]
    sentinel = getattr(session.config, _PYTEST_SENTINEL_ATTR, None)
    try:
        if sentinel is not None:
            sentinel.__exit__(None, None, None)
    finally:
        # xdist's normal finish hook has already joined its workers.
        scope = getattr(session.config, _PYTEST_DAEMON_SCOPE_ATTR, None)
        try:
            if scope is not None:
                scope.__exit__(None, None, None)
        finally:
            _report_checkout_target_leaks(session)


@pytest.hookimpl(wrapper=True)
def pytest_make_collect_report(collector):  # type: ignore[no-untyped-def]
    """Fail the collection of a module that puts Molt's stdlib on sys.path."""
    report = yield
    if isinstance(collector, pytest.Module) and _remove_molt_stdlib_top_level_root():
        report.outcome = "failed"
        report.longrepr = _MOLT_STDLIB_ROOT_LEAK.format(
            owner=collector.nodeid, root=MOLT_STDLIB_ROOT
        )
    return report


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
    artifacts = tmp_path / "implementation-artifacts"
    artifacts.mkdir()
    # The bootstrap uses DX, including in a plain source fixture. Declare the
    # fixture's external artifact root through the real boundary; no custody
    # mock or implicit sibling fallback may authorize its output.
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifacts))
    monkeypatch.setattr(
        cargo_output_layout, "implementation_source_root", lambda: source
    )
    return source

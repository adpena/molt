from __future__ import annotations

from tests.runtime_build_identity_helper import runtime_cargo_environment_inputs

import contextlib
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import runtime_native_build as runtime
from molt.cli import runtime_native_generation as generations
from molt.cli.cargo_execution import CargoExecutionResult
from molt.cli.models import _RuntimeArtifactState
from molt.cli.product_store import ProductStore
from molt.cli.native_link_manifest import native_link_dependency_manifest_path
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.exact_json import canonical_json_sha256
from molt.process_environment import ProcessEnvironment
from tests.cli.native_link_test_support import write_test_static_archive
from tests.runtime_build_identity_helper import (
    RUNTIME_FIXTURE_INPUT,
    native_runtime_staticlib_identity,
    runtime_cargo_plan,
    runtime_cargo_stdout,
    write_runtime_crate,
)


def publish(coordinate: Path, *, seed: str = "one", inputs_are_current=lambda: True):
    coordinate.parent.mkdir(parents=True, exist_ok=True)
    scratch = coordinate.with_name("cargo-output.a")
    write_test_static_archive(scratch, payload=seed.encode())
    return generations.publish_native_runtime_generation(
        coordinate,
        source_archive=scratch,
        cargo_stdout="",
        cargo_stderr="note: native-static-libs: -lc\n",
        cargo_profile="dev-fast",
        target_triple=None,
        build_identity=native_runtime_staticlib_identity(
            cargo_profile="dev-fast", family_seed=seed
        ),
        inputs_are_current=inputs_are_current,
        cargo_environment=runtime_cargo_environment_inputs(),
    )


def read(coordinate: Path):
    return generations.read_native_runtime_generation(
        coordinate, cargo_profile="dev-fast", target_triple=None
    )


def stored_selector(store: ProductStore, generation) -> Path:
    """The store selection receipt that admits ``generation``."""
    variant = store.variant(
        generation.build_identity.digest, generation.cargo_environment
    )
    return generations.native_runtime_generation_path(
        variant / generation.runtime_lib.name
    )


def test_native_selection_is_written_once_and_ignores_cargo_alias(tmp_path):
    """A receipt is never replaced: a reader may hold what it selects."""
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    first = publish(coordinate)
    assert first is not None and first.runtime_lib != coordinate
    assert not coordinate.exists()
    receipt = generations.native_runtime_generation_path(coordinate).read_bytes()
    with pytest.raises(FileExistsError):
        publish(coordinate, seed="two")
    first.verify()
    assert generations.native_runtime_generation_path(coordinate).read_bytes() == (
        receipt
    )
    assert read(coordinate).runtime_lib == first.runtime_lib
    write_test_static_archive(coordinate, payload=b"untrusted alias")
    assert read(coordinate).runtime_lib == first.runtime_lib


@pytest.mark.parametrize("member", ["archive", "manifest"])
@pytest.mark.parametrize("mutation", ["missing", "rewrite", "replace"])
def test_native_generation_rejects_partial_or_mutated_members(
    tmp_path, member, mutation
):
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    admitted = publish(coordinate)
    assert admitted is not None
    path = (
        admitted.runtime_lib
        if member == "archive"
        else native_link_dependency_manifest_path(admitted.runtime_lib)
    )
    before = path.stat()
    data = path.read_bytes()
    if mutation == "missing":
        path.unlink()
    elif mutation == "replace":
        replacement = path.with_suffix(path.suffix + ".replacement")
        replacement.write_bytes(data)
        os.replace(replacement, path)
    else:
        path.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises((OSError, ValueError)):
        admitted.verify()
    if mutation != "replace":
        with pytest.raises(ValueError, match="does not admit"):
            read(coordinate)
    else:
        # A cold admission may accept the same bytes; the old live operation
        # must reject the replaced generation even with identical content.
        assert read(coordinate) is not None


@pytest.mark.parametrize("failure", ["inputs", "manifest", "directory", "selector"])
def test_a_failed_native_publication_leaves_no_selection(
    tmp_path, monkeypatch, failure
):
    first = publish(tmp_path / "first" / "dev-fast" / "libmolt_runtime.micro.a")
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"

    def fail(*_args, **_kwargs):
        raise OSError("publication fault")

    if failure == "manifest":
        monkeypatch.setattr(generations, "write_native_link_dependency_manifest", fail)
    elif failure == "directory":
        monkeypatch.setattr(generations, "durable_publish_directory_exclusive", fail)
    elif failure == "selector":
        monkeypatch.setattr(generations, "atomic_write_bytes", fail)
    if failure == "inputs":
        assert publish(coordinate, seed="two", inputs_are_current=lambda: False) is None
    else:
        with pytest.raises(OSError, match="publication fault"):
            publish(coordinate, seed="two")
    assert not generations.native_runtime_generation_path(coordinate).exists()
    assert read(coordinate) is None
    first.verify()


def test_native_selection_does_not_admit_incomplete_or_redirected_receipt(tmp_path):
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    first = publish(coordinate)
    selector = generations.native_runtime_generation_path(coordinate)
    payload = json.loads(selector.read_text(encoding="utf-8"))
    payload["generation"] = "0" * 64
    selector.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="does not name its own material"):
        read(coordinate)
    selector.unlink()
    write_test_static_archive(coordinate)
    assert read(coordinate) is None
    first.verify()


@pytest.fixture
def native_build(tmp_path, runtime_fixture_root, monkeypatch):
    source = write_runtime_crate(tmp_path)
    captures = []
    commands = []
    plans = []
    cargo_runs = []
    events = []
    current = SimpleNamespace(coordinate=None, environment={})

    @contextlib.contextmanager
    def lock(_root, _name):
        events.append("lock")
        yield

    def resolve(*_args, **kwargs):
        commands.append(list(kwargs["cargo_command"]))
        plan = runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env=kwargs["env"],
            cargo_command=kwargs["cargo_command"],
            requested_target=kwargs["requested_target"],
            environment_transform=kwargs["environment_transform"],
            rustflags_transform=kwargs["rustflags_transform"],
        )
        plans.append(plan)
        return plan

    def identity(*_args, **kwargs):
        assert kwargs["cargo_plan"].command == plans[-1].command
        assert {
            key: value
            for key, value in kwargs["cargo_plan"].environment.items()
            if key != "MOLT_BUILD_PYTHON_SIGNATURE"
        } == dict(plans[-1].environment)
        assert tuple(kwargs["cargo_command"]) == plans[-1].command
        captures.append(tuple(kwargs["runtime_features"]))
        events.append("capture")
        seed = hashlib.sha256(source.read_bytes()).hexdigest() + repr(captures[-1])
        return native_runtime_staticlib_identity(
            cargo_profile="dev-fast", family_seed=seed
        )

    def cargo(plan, **_kwargs):
        assert plan.command == plans[-1].command
        assert plan.environment["MOLT_BUILD_PYTHON_SIGNATURE"] == canonical_json_sha256(
            native_runtime_staticlib_identity().toolchain_manifest.to_dict()["payload"][
                "toolchain"
            ]["tools"]["build_python"]
        )
        cargo_runs.append(plan)
        scratch = runtime._runtime_cargo_scratch_lib_path(current.coordinate, None)
        scratch.parent.mkdir(parents=True, exist_ok=True)
        write_test_static_archive(
            scratch, payload=repr((source.read_bytes(), captures[-1])).encode()
        )
        return CargoExecutionResult(
            subprocess.CompletedProcess(
                plan.command,
                0,
                runtime_cargo_stdout(tmp_path, (scratch,), plan.environment),
                "note: native-static-libs: -lc\n",
            ),
            attempts=(),
            retry_reason=None,
        )

    monkeypatch.setattr(
        runtime, "select_installed_native_runtime", lambda *_a, **_k: None
    )
    monkeypatch.setattr(
        runtime,
        "_cargo_build_env",
        lambda: {
            "MOLT_BUILD_PYTHON": os.path.relpath(sys.executable, tmp_path),
            **current.environment,
        },
    )
    monkeypatch.setattr(runtime, "_cargo_target_root", lambda _root: tmp_path / "cargo")
    store = ProductStore(tmp_path / "store" / "native-runtime")
    monkeypatch.setattr(runtime, "native_runtime_store", lambda _root: store)
    monkeypatch.setattr(runtime, "_build_state_root", lambda _root: tmp_path / "state")
    monkeypatch.setattr(runtime, "_build_slot", lambda: contextlib.nullcontext())
    monkeypatch.setattr(runtime, "_build_lock", lock)
    monkeypatch.setattr(runtime, "resolve_runtime_cargo_plan", resolve)
    monkeypatch.setattr(runtime, "_runtime_build_identity_for_plan", identity)
    monkeypatch.setattr(runtime, "_run_resolved_cargo_plan", cargo)

    def run(
        *,
        profile="micro",
        features=(),
        modules=(),
        coordinate=None,
        rust_cpu=RustCpuPolicy.BASELINE,
        cargo_timeout=1,
    ):
        current.coordinate = (
            coordinate or tmp_path / "cargo" / "dev-fast" / f"runtime.{profile}.a"
        )
        state = _RuntimeArtifactState(runtime_lib=current.coordinate, rust_cpu=rust_cpu)
        timings = {}
        ok = runtime._ensure_runtime_lib(
            current.coordinate,
            None,
            True,
            "dev-fast",
            tmp_path,
            cargo_timeout,
            stdlib_profile=profile,
            extra_runtime_features=features,
            resolved_modules=modules,
            rust_cpu=state.rust_cpu,
            runtime_state=state,
            stage_timings_ms=timings,
        )
        return ok, state, timings

    return SimpleNamespace(
        run=run,
        source=source,
        captures=captures,
        commands=commands,
        plans=plans,
        cargo_runs=cargo_runs,
        events=events,
        root=tmp_path,
        current=current,
        store=store,
    )


def test_native_runtime_cargo_plan_carries_the_build_rust_cpu_policy(native_build):
    ok, _state, _timings = native_build.run(rust_cpu=RustCpuPolicy.NATIVE)
    assert ok
    native = native_build.cargo_runs[-1]
    ok, _state, _timings = native_build.run(
        rust_cpu=RustCpuPolicy.BASELINE, profile="standard"
    )
    assert ok
    baseline = native_build.plans[-1]

    # Cargo executes the flags the plan pins, never a process RUSTFLAGS.
    assert native.rustflags[-2:] == ("-C", "target-cpu=native")
    assert native.environment["CARGO_ENCODED_RUSTFLAGS"].endswith("target-cpu=native")
    assert "target-cpu=native" not in baseline.rustflags
    assert "target-cpu=native" not in baseline.environment["CARGO_ENCODED_RUSTFLAGS"]


def test_native_runtime_python_signature_reaches_the_actual_executor(native_build):
    ok, _state, _timings = native_build.run()
    assert ok and len(native_build.cargo_runs) == 1
    assert native_build.cargo_runs[0].environment["MOLT_BUILD_PYTHON_SIGNATURE"]
    executable = Path(native_build.cargo_runs[0].environment["MOLT_BUILD_PYTHON"])
    assert executable.is_absolute()
    assert executable.samefile(sys.executable)
    assert "MOLT_BUILD_PYTHON_SIGNATURE" not in native_build.plans[0].environment
    assert len(native_build.captures) == 2


def test_native_build_captures_twice_and_retained_admission_once(native_build):
    ok, first, timings = native_build.run()
    assert ok and len(native_build.captures) == 2
    assert native_build.events[0] == "lock"
    assert len(native_build.cargo_runs) == 1
    assert native_build.cargo_runs[0] is not native_build.plans[0]
    assert "MOLT_BUILD_PYTHON_SIGNATURE" not in native_build.plans[0].environment
    assert {
        "runtime_lib_identity_initial",
        "runtime_lib_identity_publication",
    } <= timings.keys()
    assert all(value >= 0.0 for value in timings.values())
    ok, second, timings = native_build.run(
        modules=("socket", "serial", "zlib", "molt.gpu")
    )
    assert ok and second.runtime_lib == first.runtime_lib
    assert len(native_build.captures) == 3 and len(native_build.cargo_runs) == 1
    assert all(
        features == native_build.captures[0] for features in native_build.captures
    )
    assert {"builtin_set", "stdlib_micro", "no-default-features"} <= set(
        native_build.captures[0]
    )
    assert not {
        "stdlib_net",
        "stdlib_serial",
        "stdlib_compression",
        "molt_gpu_primitives",
    }.intersection(native_build.captures[0])
    assert set(timings) == {
        "runtime_lib_generation_read",
        "runtime_lib_identity_initial",
    }
    assert all(value >= 0.0 for value in timings.values())


def test_native_admission_observes_source_rewrite_with_restored_mtime(native_build):
    assert native_build.run()[0]
    before = native_build.source.stat()
    native_build.source.write_bytes(b"after!")
    os.utime(native_build.source, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert native_build.run()[0]
    assert len(native_build.cargo_runs) == 2


@pytest.mark.parametrize("before,after", [(None, ""), ("", "changed"), ("set", None)])
@pytest.mark.parametrize("rebuild", [False, True])
def test_native_admission_reverifies_observed_environment_without_cargo_on_reuse(
    native_build, monkeypatch, before, after, rebuild
):
    def change(value):
        native_build.current.environment.clear()
        if value is not None:
            native_build.current.environment[RUNTIME_FIXTURE_INPUT] = value

    change(before)
    ok, original, _timings = native_build.run()
    assert ok and len(native_build.cargo_runs) == 1
    generation = original.native_runtime_generation
    selector = stored_selector(native_build.store, generation)
    retained = selector.read_bytes()
    ok, warm, _timings = native_build.run()
    assert ok and warm.runtime_lib == original.runtime_lib
    assert len(native_build.cargo_runs) == 1
    assert selector.read_bytes() == retained

    change(after)
    if not rebuild:
        monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    ok, changed, _timings = native_build.run()
    assert ok is rebuild
    if rebuild:
        assert len(native_build.cargo_runs) == 2
        assert (
            changed.native_runtime_build_identity
            == original.native_runtime_build_identity
        )
        # This fixture deliberately emits equal code. Input changes must not
        # fabricate different archive identities, but their receipts must differ.
        assert changed.runtime_lib.read_bytes() == original.runtime_lib.read_bytes()
        assert (
            changed.native_runtime_generation.cargo_environment
            != generation.cargo_environment
        )
    else:
        assert len(native_build.cargo_runs) == 1
        assert changed.native_runtime_generation is None
        assert changed.native_runtime_build_failure.stage == "rebuild-policy"
        assert selector.read_bytes() == retained
    generation.verify()


def test_native_publication_rejects_inputs_changed_during_staging(
    native_build, monkeypatch
):
    write_manifest = generations.write_native_link_dependency_manifest

    def mutate(*args, **kwargs):
        result = write_manifest(*args, **kwargs)
        native_build.source.write_bytes(b"changed while staging")
        return result

    monkeypatch.setattr(generations, "write_native_link_dependency_manifest", mutate)
    ok, state, _timings = native_build.run()
    assert not ok and state.native_runtime_build_identity is None
    assert (
        state.native_runtime_build_failure.stage
        == "generation-publication-identity-stability"
    )
    assert not any(native_build.store.root.glob("*/*/*.generation.json"))


@pytest.mark.parametrize("profile", ["micro", "full"])
@pytest.mark.parametrize(
    "features",
    [(), ("stdlib_tk",), ("molt_gpu_primitives",), ("source_extension_loader",)],
)
def test_native_generation_identity_keeps_profile_and_feature_authority(
    native_build, monkeypatch, profile, features
):
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    assert native_build.run(profile=profile, features=features)[0]
    authored = set(native_build.captures[0])
    assert {"molt_tk_native", *features} <= authored
    command = native_build.commands[0]
    assert command[:8] == [
        "cargo",
        "rustc",
        "--color=never",
        "-p",
        "molt-runtime",
        "--profile",
        "dev-fast",
        "--message-format=json-render-diagnostics",
    ]
    assert command[-5:] == [
        "--crate-type",
        "staticlib",
        "--",
        "--print",
        "native-static-libs",
    ]
    cargo_features = set(command[command.index("--features") + 1].split(","))
    assert {"molt_tk_native", *features} <= cargo_features
    if profile == "full":
        assert {"stdlib_full", "default-features"} <= authored
        assert "no-default-features" not in authored
        assert "--no-default-features" not in command
        assert "stdlib_full" in cargo_features
        assert "stdlib_micro" not in cargo_features
    else:
        assert {"builtin_set", "stdlib_micro", "no-default-features"} <= authored
        assert "default-features" not in authored
        assert "--no-default-features" in command
    assert native_build.run(profile=profile, features=features)[0]
    assert len(native_build.cargo_runs) == 1


def test_native_profiles_retain_their_own_generation_across_cargo_overwrite(
    native_build,
):
    ok, micro, _timings = native_build.run(profile="micro")
    assert ok
    micro_bytes = micro.runtime_lib.read_bytes()
    ok, full, _timings = native_build.run(profile="full")
    assert ok and full.runtime_lib != micro.runtime_lib
    full_bytes = full.runtime_lib.read_bytes()
    assert full_bytes != micro_bytes
    for _ in range(2):
        ok, retained, _timings = native_build.run(profile="micro")
        assert ok and retained.runtime_lib == micro.runtime_lib
        assert retained.runtime_lib.read_bytes() == micro_bytes
    assert full.runtime_lib.read_bytes() == full_bytes
    assert len(native_build.cargo_runs) == 2


def test_another_target_reuses_the_stored_generation_without_cargo(native_build):
    first = native_build.root / "cargo" / "dev-fast" / "runtime.micro.a"
    ok, stored, _timings = native_build.run(coordinate=first)
    assert ok and len(native_build.cargo_runs) == 1
    # Another checkout's or session's target names another coordinate.
    second = native_build.root / "other-target" / "dev-fast" / "runtime.micro.a"
    ok, reused, _timings = native_build.run(coordinate=second)
    assert ok and len(native_build.cargo_runs) == 1
    assert reused.runtime_lib == stored.runtime_lib
    assert reused.runtime_lib.is_relative_to(native_build.store.root)
    assert not first.exists() and not second.exists()


def test_a_build_that_waited_for_the_lock_reuses_what_the_holder_published(
    native_build, monkeypatch
):
    other = native_build.root / "other-checkout" / "dev-fast" / "runtime.micro.a"
    original = ProductStore.single_flight
    contended = []

    @contextlib.contextmanager
    def single_flight(self, identity):
        if not contended:
            contended.append(identity)
            # Another checkout with the same inputs builds and publishes
            # while this build waits for the identity's lock.
            assert native_build.run(coordinate=other)[0]
        with original(self, identity):
            yield

    monkeypatch.setattr(ProductStore, "single_flight", single_flight)
    ok, state, _timings = native_build.run()
    assert ok and contended
    assert len(native_build.cargo_runs) == 1
    assert state.runtime_lib.is_relative_to(native_build.store.root)


def test_a_waiter_outwaits_its_own_cargo_timeout_while_the_holder_builds(
    native_build, monkeypatch, capsys
):
    """The identity lock waits while its holder builds (design 56, 2.8).

    The holder here keeps the lock six times this build's Cargo timeout, as a
    backend build holds it for Cargo, its probe and publication. A waiter
    bounded by its Cargo timeout would fail at stage build-lock; this one
    waits, says once whom it waits for, and then builds.
    """
    import threading

    from molt.cli.build_locks import held_build_lock

    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    original = ProductStore.single_flight
    held, release = threading.Event(), threading.Event()

    def holder(lock: Path) -> None:
        with held_build_lock(lock, purpose="the other checkout's build"):
            held.set()
            release.wait(timeout=30)

    @contextlib.contextmanager
    def single_flight(self, identity):
        lock = self.build_lock(identity)
        thread = threading.Thread(target=holder, args=(lock,))
        thread.start()
        assert held.wait(timeout=10)
        timer = threading.Timer(0.6, release.set)
        timer.start()
        try:
            with original(self, identity):
                yield
        finally:
            timer.join()
            thread.join(timeout=30)

    monkeypatch.setattr(ProductStore, "single_flight", single_flight)
    ok, state, _timings = native_build.run(cargo_timeout=0.1)
    assert ok, state.native_runtime_build_failure
    assert len(native_build.cargo_runs) == 1
    notices = [
        line for line in capsys.readouterr().err.splitlines() if "Blocking" in line
    ]
    assert len(notices) == 1 and f"pid {os.getpid()}," in notices[0]


@pytest.mark.parametrize("corruption", ["archive", "manifest", "receipt"])
def test_a_corrupt_stored_generation_fails_loud_without_cargo(native_build, corruption):
    ok, stored, _timings = native_build.run()
    assert ok and len(native_build.cargo_runs) == 1
    generation = stored.native_runtime_generation
    if corruption == "archive":
        write_test_static_archive(generation.runtime_lib, payload=b"corrupt entry")
    elif corruption == "manifest":
        native_link_dependency_manifest_path(generation.runtime_lib).unlink()
    else:
        selector = stored_selector(native_build.store, generation)
        payload = json.loads(selector.read_text(encoding="utf-8"))
        payload["generation"] = "0" * 64
        selector.write_text(json.dumps(payload), encoding="utf-8")
    ok, state, _timings = native_build.run()
    assert not ok
    failure = state.native_runtime_build_failure
    assert failure.stage == "product-store"
    assert "is corrupt" in failure.summary
    assert f"molt cache evict native-runtime {generation.build_identity.digest}" in (
        failure.summary
    )
    # A receipt whose environment can be read is this build's own variant and
    # fails before Cargo; one that cannot names no environment, so the build
    # runs and its publication finds the receipt it would have written.
    assert len(native_build.cargo_runs) == (1 if corruption == "archive" else 2)
    # The named tool removes the entry and the next build publishes it again.
    assert native_build.store.evict(generation.build_identity.digest)
    ok, rebuilt, _timings = native_build.run()
    assert ok and rebuilt.native_runtime_generation is not None
    assert len(native_build.cargo_runs) == (2 if corruption == "archive" else 3)


def test_a_corrupt_variant_of_another_environment_never_fails_a_reader(
    native_build,
):
    """Only a reader's own variant is read; another's corruption is not its own."""
    native_build.current.environment[RUNTIME_FIXTURE_INPUT] = "one"
    ok, one, _timings = native_build.run()
    assert ok
    write_test_static_archive(
        one.native_runtime_generation.runtime_lib, payload=b"corrupt entry"
    )
    native_build.current.environment[RUNTIME_FIXTURE_INPUT] = "two"
    ok, two, _timings = native_build.run()
    assert ok and len(native_build.cargo_runs) == 2
    ok, warm, _timings = native_build.run()
    assert ok and warm.runtime_lib == two.runtime_lib
    assert len(native_build.cargo_runs) == 2
    # The corrupt variant still fails its own environment, before Cargo.
    native_build.current.environment[RUNTIME_FIXTURE_INPUT] = "one"
    ok, again, _timings = native_build.run()
    assert not ok and again.native_runtime_build_failure.stage == "product-store"
    assert len(native_build.cargo_runs) == 2


@pytest.mark.parametrize("state", ["valid", "missing", "stale", "partial"])
def test_disabled_native_rebuild_still_requires_a_complete_current_generation(
    native_build, monkeypatch, state
):
    ok, admitted, _timings = native_build.run()
    assert ok
    if state == "missing":
        # A variant whose receipt has not appeared is an unfinished publication.
        stored_selector(native_build.store, admitted.native_runtime_generation).unlink()
    elif state == "stale":
        native_build.source.write_bytes(b"new source")
    elif state == "partial":
        native_link_dependency_manifest_path(admitted.runtime_lib).unlink()
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    ok, current, _timings = native_build.run()
    assert ok == (state == "valid")
    assert len(native_build.cargo_runs) == 1
    if not ok:
        # A variant without its manifest names no environment, so it is
        # not this build's; the rebuild it would need is refused by policy.
        assert current.native_runtime_build_failure.stage == "rebuild-policy"


def test_unreceipted_alias_never_authorizes_native_reuse(native_build):
    coordinate = native_build.root / "cargo" / "dev-fast" / "runtime.micro.a"
    coordinate.parent.mkdir(parents=True)
    write_test_static_archive(coordinate)
    ok, state, _timings = native_build.run()
    assert ok and state.runtime_lib != coordinate
    assert len(native_build.cargo_runs) == 1


def test_native_gpu_feature_change_rebuilds_the_selected_generation(
    native_build, monkeypatch
):
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "0")
    ok, first, _timings = native_build.run()
    assert ok
    first_bytes = first.runtime_lib.read_bytes()
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    ok, second, _timings = native_build.run()
    assert ok and second.runtime_lib != first.runtime_lib
    assert len(native_build.cargo_runs) == 2
    assert "molt_gpu_metal" in native_build.captures[-1]
    command = native_build.commands[-1]
    assert "molt_gpu_metal" in command[command.index("--features") + 1].split(",")
    ok, retained, _timings = native_build.run()
    assert ok and retained.runtime_lib == second.runtime_lib
    assert len(native_build.cargo_runs) == 2
    assert first.runtime_lib.read_bytes() == first_bytes


def _transport(coordinate: Path, destination: Path) -> tuple[Path, dict[str, Path]]:
    """Copy one selection and its members the way a transport would."""
    admitted = read(coordinate)
    assert admitted is not None
    destination.mkdir(parents=True)
    selection = destination / "selection.json"
    selection.write_bytes(
        generations.native_runtime_generation_path(coordinate).read_bytes()
    )
    members = {}
    for role, identity in admitted.members:
        copy = destination / role
        copy.write_bytes(identity.path.read_bytes())
        members[role] = copy
    return selection, members


_RUNTIME_NAME = "libmolt_runtime.full.a"


def _identity(seed: str = "one"):
    return native_runtime_staticlib_identity(cargo_profile="dev-fast", family_seed=seed)


def _stored(store: ProductStore, seed: str = "one"):
    generation = generations.native_runtime_store_generation(
        store,
        _identity(seed),
        ProcessEnvironment(os.environ),
        runtime_name=_RUNTIME_NAME,
        cargo_profile="dev-fast",
        target_triple=None,
    )
    return () if generation is None else (generation,)


def _import(store: ProductStore, selection: Path, members, *, seed: str = "one"):
    return generations.import_native_runtime_generation(
        store,
        runtime_name=_RUNTIME_NAME,
        selection=selection,
        members=members,
        cargo_profile="dev-fast",
        target_triple=None,
        environment=ProcessEnvironment(os.environ),
        build_identity=_identity(seed),
    )


def test_imported_generation_reproduces_the_exported_selection(tmp_path):
    exported = tmp_path / "producer" / "dev-fast" / "libmolt_runtime.full.a"
    publish(exported)
    selection, members = _transport(exported, tmp_path / "transport")
    store = ProductStore(tmp_path / "consumer-store" / "native-runtime")

    admitted = _import(store, selection, members)

    source = read(exported)
    (stored,) = _stored(store)
    assert stored.records() == source.records()
    assert admitted.runtime_lib.parent.parent.name == (
        source.runtime_lib.parent.parent.name
    )
    assert admitted.runtime_lib.is_relative_to(store.entry(_identity().digest))


def test_import_refuses_a_generation_built_from_other_inputs(tmp_path):
    exported = tmp_path / "producer" / "dev-fast" / "libmolt_runtime.full.a"
    publish(exported)
    selection, members = _transport(exported, tmp_path / "transport")
    store = ProductStore(tmp_path / "consumer-store" / "native-runtime")

    with pytest.raises(ValueError, match="other inputs"):
        _import(store, selection, members, seed="two")
    assert not store.root.exists()


def test_import_refuses_member_bytes_that_differ_from_the_selection(tmp_path):
    exported = tmp_path / "producer" / "dev-fast" / "libmolt_runtime.full.a"
    publish(exported)
    selection, members = _transport(exported, tmp_path / "transport")
    write_test_static_archive(members["runtime_archive"], payload=b"substituted")
    store = ProductStore(tmp_path / "consumer-store" / "native-runtime")

    with pytest.raises(ValueError):
        _import(store, selection, members)
    assert not _stored(store)


def test_import_refuses_an_unportable_member_name_under_a_valid_digest(tmp_path):
    exported = tmp_path / "producer" / "dev-fast" / "libmolt_runtime.full.a"
    publish(exported)
    selection, members = _transport(exported, tmp_path / "transport")
    payload = json.loads(selection.read_text(encoding="utf-8"))
    payload["members"][1]["name"] = "../escaped.json"
    material = {key: value for key, value in payload.items() if key != "generation"}
    payload["generation"] = canonical_json_sha256(material)
    selection.write_text(json.dumps(payload), encoding="utf-8")
    store = ProductStore(tmp_path / "consumer-store" / "native-runtime")

    with pytest.raises(ValueError, match="portable"):
        _import(store, selection, members)
    assert not any(store.root.rglob("escaped.json"))
    assert not _stored(store)


class _Clock:
    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now


def test_a_reused_runtime_is_renewed_and_outlives_the_idle_window(
    native_build, monkeypatch
):
    """Reuse renews an entry (Go's trim), so one that builds keep reusing stays.

    The runtime is published on day 0 and reused on day 4. On day 6 another
    identity's publication evicts entries no build used for 5 days: the
    reused entry stays, and the next build reuses it without Cargo.
    """
    from molt.cli import generation_store, product_store

    day = 24 * 60 * 60.0
    clock = _Clock(1_000_000_000.0)
    monkeypatch.setattr(generation_store, "time", clock)
    ok, first, _timings = native_build.run(profile="micro")
    assert ok and len(native_build.cargo_runs) == 1
    entry = native_build.store.entry(
        first.native_runtime_generation.build_identity.digest
    )
    clock.now += 4 * day
    ok, reused, _timings = native_build.run(profile="micro")
    assert ok and reused.runtime_lib == first.runtime_lib
    assert len(native_build.cargo_runs) == 1
    clock.now += 2 * day
    assert 6 * day > product_store.PRODUCT_STORE_IDLE_SECONDS
    ok, _other, _timings = native_build.run(profile="full")
    assert ok and len(native_build.cargo_runs) == 2
    assert entry.is_dir()
    ok, again, _timings = native_build.run(profile="micro")
    assert ok and again.runtime_lib == first.runtime_lib
    assert len(native_build.cargo_runs) == 2

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import runtime_native_build as runtime
from molt.cli import runtime_native_generation as generations
from molt.cli.cargo_execution import CargoExecutionResult
from molt.cli.models import _RuntimeArtifactState
from molt.cli.native_link_manifest import native_link_dependency_manifest_path
from tests.cli.native_link_test_support import write_test_static_archive
from tests.runtime_build_identity_helper import (
    native_runtime_staticlib_identity,
    runtime_cargo_plan,
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
    )


def read(coordinate: Path):
    return generations.read_native_runtime_generation(
        coordinate, cargo_profile="dev-fast", target_triple=None
    )


def test_native_selection_retains_complete_generations_and_ignores_cargo_alias(
    tmp_path,
):
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    first = publish(coordinate)
    assert first is not None and first.runtime_lib != coordinate
    assert not coordinate.exists()
    second = publish(coordinate, seed="two")
    assert second is not None and second.runtime_lib != first.runtime_lib
    first.verify()
    assert read(coordinate).runtime_lib == second.runtime_lib
    write_test_static_archive(coordinate, payload=b"untrusted alias")
    assert read(coordinate).runtime_lib == second.runtime_lib


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
        assert read(coordinate) is None
    else:
        # A cold admission may accept the same bytes; the old live operation
        # must reject the replaced generation even with identical content.
        assert read(coordinate) is not None


@pytest.mark.parametrize("failure", ["inputs", "manifest", "directory", "selector"])
def test_failed_native_publication_keeps_previous_complete_selection(
    tmp_path, monkeypatch, failure
):
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    first = publish(coordinate)
    before = generations.native_runtime_generation_path(coordinate).read_bytes()

    def fail(*_args, **_kwargs):
        raise OSError("publication fault")

    if failure == "manifest":
        monkeypatch.setattr(generations, "write_native_link_dependency_manifest", fail)
    elif failure == "directory":
        monkeypatch.setattr(generations, "durable_publish_directory_exclusive", fail)
    elif failure == "selector":
        monkeypatch.setattr(generations, "_atomic_write_json", fail)
    if failure == "inputs":
        assert publish(coordinate, seed="two", inputs_are_current=lambda: False) is None
    else:
        with pytest.raises(OSError, match="publication fault"):
            publish(coordinate, seed="two")
    assert generations.native_runtime_generation_path(coordinate).read_bytes() == before
    assert read(coordinate).runtime_lib == first.runtime_lib


def test_native_selection_does_not_admit_incomplete_or_redirected_receipt(tmp_path):
    coordinate = tmp_path / "dev-fast" / "libmolt_runtime.micro.a"
    first = publish(coordinate)
    selector = generations.native_runtime_generation_path(coordinate)
    payload = json.loads(selector.read_text(encoding="utf-8"))
    payload["generation"] = "0" * 64
    selector.write_text(json.dumps(payload), encoding="utf-8")
    assert read(coordinate) is None
    selector.unlink()
    write_test_static_archive(coordinate)
    assert read(coordinate) is None
    first.verify()


@pytest.fixture
def native_build(tmp_path, runtime_fixture_root, monkeypatch):
    source = tmp_path / "source.rs"
    source.write_bytes(b"before")
    captures = []
    commands = []
    plans = []
    cargo_runs = []
    events = []
    current = SimpleNamespace(coordinate=None)

    @contextlib.contextmanager
    def lock(*_args):
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
        )
        plans.append(plan)
        return plan

    def identity(*_args, **kwargs):
        assert kwargs["cargo_plan"] is plans[-1]
        assert tuple(kwargs["cargo_command"]) == plans[-1].command
        captures.append(tuple(kwargs["runtime_features"]))
        events.append("capture")
        seed = hashlib.sha256(source.read_bytes()).hexdigest() + repr(captures[-1])
        return native_runtime_staticlib_identity(
            cargo_profile="dev-fast", family_seed=seed
        )

    def cargo(plan, **_kwargs):
        assert plan is plans[-1]
        cargo_runs.append(plan)
        scratch = runtime._runtime_cargo_scratch_lib_path(current.coordinate, None)
        scratch.parent.mkdir(parents=True, exist_ok=True)
        write_test_static_archive(
            scratch, payload=repr((source.read_bytes(), captures[-1])).encode()
        )
        return CargoExecutionResult(
            subprocess.CompletedProcess(
                plan.command, 0, "", "note: native-static-libs: -lc\n"
            ),
            attempts=(),
            retry_reason=None,
        )

    monkeypatch.setattr(
        runtime, "select_installed_native_runtime", lambda *_a, **_k: None
    )
    monkeypatch.setattr(runtime, "_cargo_build_env", lambda: {})
    monkeypatch.setattr(runtime, "_cargo_target_root", lambda _root: tmp_path / "cargo")
    monkeypatch.setattr(
        runtime, "_canonical_target_root", lambda _root: tmp_path / "canonical"
    )
    monkeypatch.setattr(runtime, "_build_state_root", lambda _root: tmp_path / "state")
    monkeypatch.setattr(runtime, "_build_slot", lambda: contextlib.nullcontext())
    monkeypatch.setattr(runtime, "_build_lock", lock)
    monkeypatch.setattr(runtime, "resolve_runtime_cargo_plan", resolve)
    monkeypatch.setattr(runtime, "_runtime_build_identity_for_plan", identity)
    monkeypatch.setattr(runtime, "_run_resolved_cargo_plan", cargo)

    def run(*, profile="micro", features=(), modules=(), coordinate=None):
        current.coordinate = (
            coordinate or tmp_path / "cargo" / "dev-fast" / f"runtime.{profile}.a"
        )
        state = _RuntimeArtifactState(runtime_lib=current.coordinate)
        timings = {}
        ok = runtime._ensure_runtime_lib(
            current.coordinate,
            None,
            True,
            "dev-fast",
            tmp_path,
            1,
            stdlib_profile=profile,
            extra_runtime_features=features,
            resolved_modules=modules,
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
    )


def test_native_build_captures_twice_and_retained_admission_once(native_build):
    ok, first, timings = native_build.run()
    assert ok and len(native_build.captures) == 2
    assert native_build.events[0] == "lock"
    assert len(native_build.cargo_runs) == 1
    assert native_build.cargo_runs[0] is native_build.plans[0]
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
    assert not generations.native_runtime_generation_path(
        native_build.current.coordinate
    ).exists()


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


@pytest.mark.parametrize("corruption", [None, "archive", "manifest"])
def test_native_admission_consumes_only_valid_canonical_generations(
    native_build, corruption
):
    coordinate = native_build.root / "canonical" / "dev-fast" / "runtime.micro.a"
    ok, canonical, _timings = native_build.run(coordinate=coordinate)
    assert ok
    if corruption == "archive":
        write_test_static_archive(canonical.runtime_lib, payload=b"corrupt canonical")
    elif corruption == "manifest":
        native_link_dependency_manifest_path(canonical.runtime_lib).unlink()
    ok, isolated, _timings = native_build.run()
    assert ok
    if corruption is None:
        assert isolated.runtime_lib == canonical.runtime_lib
        assert len(native_build.cargo_runs) == 1
    else:
        assert isolated.runtime_lib != canonical.runtime_lib
        assert len(native_build.cargo_runs) == 2
        assert read(native_build.current.coordinate).runtime_lib == isolated.runtime_lib
    assert not native_build.current.coordinate.exists()


@pytest.mark.parametrize("state", ["valid", "missing", "stale", "partial"])
def test_disabled_native_rebuild_still_requires_a_complete_current_generation(
    native_build, monkeypatch, state
):
    ok, admitted, _timings = native_build.run()
    assert ok
    if state == "missing":
        generations.native_runtime_generation_path(
            native_build.current.coordinate
        ).unlink()
    elif state == "stale":
        native_build.source.write_bytes(b"new source")
    elif state == "partial":
        native_link_dependency_manifest_path(admitted.runtime_lib).unlink()
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    ok, current, _timings = native_build.run()
    assert ok == (state == "valid")
    assert len(native_build.cargo_runs) == 1
    if not ok:
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

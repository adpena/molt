"""Content-addressed reuse of final WASM link results."""

from __future__ import annotations

import hashlib
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import BinaryIO

import pytest

from molt import artifact_publication, process_guard, toolchain_identity
from molt.cli import wasm_final_link_cache as cache
from molt.cli import wasm_link_cache


def _link_args(*, out_dir: Path, app: Path, timings: str, split_dir: str) -> list[str]:
    return [
        "--input",
        str(app),
        "--output",
        str(out_dir / "hello_linked.wasm"),
        "--split-output-dir",
        split_dir,
        "--phase-timings-file",
        timings,
        "--optimize-level",
        "O1",
    ]


def _miss() -> None:
    raise RuntimeError("cache miss")


def _inputs(tmp_path: Path) -> Path:
    app = tmp_path / "app.wasm"
    app.write_bytes(b"\0asm\x01\0\0\0app-v1")
    return app


def test_key_ignores_private_locations_and_follows_input_content(
    tmp_path: Path,
) -> None:
    app = _inputs(tmp_path)
    facts = ({"role": "wasm-link-source-closure", "content_digest": "a" * 64},)

    def key(out: str, timings: str, split: str) -> str:
        return cache.final_link_cache_key(
            _link_args(
                out_dir=tmp_path / out,
                app=app,
                timings=timings,
                split_dir=split,
            ),
            cwd=tmp_path,
            tool_facts=facts,
        )

    first = key("ci-row-a", "/tmp/.molt-link-timings-1.tmp", "/tmp/gen-1")
    # A fresh output directory, generation directory and timings file: one result.
    assert key("ci-row-b", "/tmp/.molt-link-timings-2.tmp", "/tmp/gen-2") == first
    # Changed input bytes at the same path: a different result.
    app.write_bytes(b"\0asm\x01\0\0\0app-v2")
    assert key("ci-row-a", "/tmp/.molt-link-timings-1.tmp", "/tmp/gen-1") != first


@pytest.mark.skipif(os.name == "nt", reason="settled POSIX file observation")
@pytest.mark.parametrize("admit_before_key", [False, True])
def test_repeated_input_uses_one_exact_capture_and_rejects_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, admit_before_key: bool
) -> None:
    app = _inputs(tmp_path)
    # Observe actual streams without waiting for a test file's timestamp to age.
    monkeypatch.setattr(toolchain_identity, "DIGEST_RACY_WINDOW_NS", 0)
    reads = []
    stream_digest = toolchain_identity._sha256_stream

    def digest(stream: BinaryIO, *, max_bytes: int) -> str:
        reads.append(os.fstat(stream.fileno()).st_ino)
        return stream_digest(stream, max_bytes=max_bytes)

    monkeypatch.setattr(toolchain_identity, "_sha256_stream", digest)
    obs = toolchain_identity.StableRegularFileObservation.capture(app, label="app")
    observations = {str(app): obs} if admit_before_key else None
    reads.clear()
    cmd = _link_args(out_dir=tmp_path, app=app, timings="t", split_dir="s")
    cmd.extend(
        ("--expected-input", app.name, hashlib.sha256(app.read_bytes()).hexdigest())
    )
    key = cache.final_link_cache_key(
        cmd, cwd=tmp_path, tool_facts=(), observed_inputs=observations
    )
    assert reads == ([] if admit_before_key else [app.stat().st_ino])
    original = app.stat()
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"\0asm\x01\0\0\0app-v2")
    replacement.replace(app)
    os.utime(app, ns=(original.st_atime_ns, original.st_mtime_ns))
    if admit_before_key:
        with pytest.raises(ValueError, match="changed since identity capture"):
            cache.final_link_cache_key(
                cmd, cwd=tmp_path, tool_facts=(), observed_inputs=observations
            )
    assert cache.final_link_cache_key(cmd, cwd=tmp_path, tool_facts=()) != key


def test_key_keeps_the_embedded_module_name_and_hashes_relative_inputs(
    tmp_path: Path,
) -> None:
    app = _inputs(tmp_path)
    facts = ({"role": "wasm-link-source-closure", "content_digest": "a" * 64},)
    base = _link_args(out_dir=tmp_path, app=app, timings="t", split_dir="s")
    renamed = list(base)
    renamed[renamed.index("--output") + 1] = str(tmp_path / "other_linked.wasm")
    relative = list(base)
    relative[relative.index("--input") + 1] = app.name
    key = cache.final_link_cache_key(base, cwd=tmp_path, tool_facts=facts)
    # wasm-ld embeds the output file name in the module.
    assert cache.final_link_cache_key(renamed, cwd=tmp_path, tool_facts=facts) != key
    # The tool resolves a relative input against its cwd; it is keyed by content.
    assert cache.final_link_cache_key(relative, cwd=tmp_path, tool_facts=facts) == key
    other_facts = ({"role": "wasm-link-source-closure", "content_digest": "b" * 64},)
    assert cache.final_link_cache_key(base, cwd=tmp_path, tool_facts=other_facts) != key


def test_result_roundtrips_and_a_tampered_entry_is_a_miss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    entry = cache.final_link_cache_entry("c" * 64)
    produced = {
        "linked": tmp_path / "gen-a" / "hello_linked.wasm",
        "optimizer": tmp_path / "gen-a" / "hello_linked.wasm.wasm-opt.json",
    }
    for role, path in produced.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(f"{role} bytes".encode())
    fresh = {role: tmp_path / "gen-b" / path.name for role, path in produced.items()}

    assert not cache.ensure_final_link_result(entry, produced, lambda: None).reused
    assert cache.ensure_final_link_result(
        entry, fresh, lambda: pytest.fail("unexpected producer")
    ).reused
    assert {role: path.read_bytes() for role, path in fresh.items()} == {
        role: path.read_bytes() for role, path in produced.items()
    }

    # A request for a different output family never matches this entry.
    with pytest.raises(RuntimeError, match="cache miss"):
        cache.ensure_final_link_result(entry, {"linked": fresh["linked"]}, _miss)

    cache.ensure_final_link_result(entry, produced, lambda: None)
    (entry.root / "roles" / "linked").write_bytes(b"tampered")
    again = {role: tmp_path / "gen-c" / path.name for role, path in produced.items()}
    with pytest.raises(RuntimeError, match="cache miss"):
        cache.ensure_final_link_result(entry, again, _miss)
    assert not any(path.exists() for path in again.values())
    assert not entry.metadata.exists()


@pytest.mark.parametrize("operation", ["publish", "restore"])
def test_bundle_transfer_preserves_a_colliding_unowned_stage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    source = tmp_path / "source.wasm"
    source.write_bytes(b"complete generation")
    entry = cache.final_link_cache_entry("d" * 64)
    cache.ensure_final_link_result(entry, {"linked": source}, lambda: None)
    prior_metadata = entry.metadata.read_bytes()
    destination = tmp_path / "result.wasm"
    destination.write_bytes(b"previous output")
    stage = tmp_path / "rival-stage"
    stage.write_bytes(b"another owner's bytes")
    prior_stage = stage.stat()
    monkeypatch.setattr(
        artifact_publication, "staged_output_path", lambda *_args, **_kwargs: stage
    )
    with pytest.raises(FileExistsError):
        if operation == "publish":
            with cache._locked_wasm_link_cache_entry(entry):
                wasm_link_cache._publish_wasm_link_cache_bundle(
                    entry, {"linked": source}
                )
        else:
            cache.ensure_final_link_result(entry, {"linked": destination}, _miss)
    assert stage.read_bytes() == b"another owner's bytes"
    assert stage.stat().st_ino == prior_stage.st_ino
    assert destination.read_bytes() == b"previous output"
    assert source.read_bytes() == b"complete generation"
    assert entry.metadata.read_bytes() == prior_metadata


@pytest.mark.parametrize("operation", ["publish", "restore"])
def test_bundle_transfer_rejects_bytes_changed_after_admission(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    source = tmp_path / "source.wasm"
    source.write_bytes(b"original generation")
    entry = cache.final_link_cache_entry("e" * 64)
    cache.ensure_final_link_result(entry, {"linked": source}, lambda: None)
    metadata = entry.metadata.read_bytes()
    destination = tmp_path / "result.wasm"
    destination.write_bytes(b"previous output")
    cached_role = entry.root / "roles" / "linked"
    target = source if operation == "publish" else cached_role
    capture = wasm_link_cache.stable_regular_file_identity
    mutations = []

    def mutate_after_capture(path, *, label):
        admitted = capture(path, label=label)
        if path == target:
            replacement = tmp_path / "replacement"
            replacement.write_bytes(b"substituted payload")
            replacement.replace(path)
            mutations.append(path)
        return admitted

    monkeypatch.setattr(
        wasm_link_cache, "stable_regular_file_identity", mutate_after_capture
    )
    with pytest.raises(ValueError, match="source changed while staging verified copy"):
        if operation == "publish":
            with cache._locked_wasm_link_cache_entry(entry):
                wasm_link_cache._publish_wasm_link_cache_bundle(
                    entry, {"linked": source}
                )
        else:
            cache.ensure_final_link_result(entry, {"linked": destination}, _miss)
    assert mutations == [target]
    assert destination.read_bytes() == b"previous output"
    assert entry.metadata.read_bytes() == metadata
    if operation == "publish":
        assert cached_role.read_bytes() == b"original generation"


def test_failure_evidence_destination_is_not_a_link_content_input(
    tmp_path: Path,
) -> None:
    app = _inputs(tmp_path)
    command = _link_args(out_dir=tmp_path, app=app, timings="t", split_dir="s")
    facts = ({"role": "wasm-link-source-closure", "content_digest": "a" * 64},)
    expected = cache.final_link_cache_key(command, cwd=tmp_path, tool_facts=facts)
    for destination in (tmp_path / "first", tmp_path / "second"):
        destination.mkdir()
        assert (
            cache.final_link_cache_key(
                [*command, "--failure-evidence-dir", str(destination)],
                cwd=tmp_path,
                tool_facts=facts,
            )
            == expected
        )
    changed_source = ({"role": "wasm-link-source-closure", "content_digest": "b" * 64},)
    assert (
        cache.final_link_cache_key(command, cwd=tmp_path, tool_facts=changed_source)
        != expected
    )


@pytest.mark.parametrize("first_fails", [False, True])
def test_matching_requests_have_one_locked_producer_and_retry_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first_fails: bool
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    entry = cache.final_link_cache_entry("7" * 64)
    entered, release, waiter_started = (
        threading.Event(),
        threading.Event(),
        threading.Event(),
    )
    calls: list[str] = []
    first = {role: tmp_path / "first" / role for role in ("linked", "optimizer")}
    second = {role: tmp_path / "second" / role for role in first}

    def write(outputs: dict[str, Path]) -> None:
        for role, path in outputs.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((role + " admitted bytes").encode())

    def produce_first() -> None:
        calls.append("first")
        entered.set()
        assert release.wait(300)
        if first_fails:
            first["linked"].parent.mkdir(parents=True, exist_ok=True)
            first["linked"].write_bytes(b"partial rejected product")
            raise ValueError("first producer rejected")
        write(first)

    def produce_second() -> None:
        calls.append("second")
        assert not entry.metadata.exists()
        write(second)

    def waiter() -> cache.FinalLinkResult:
        waiter_started.set()
        return cache.ensure_final_link_result(entry, second, produce_second)

    with ThreadPoolExecutor(max_workers=2) as executor:
        owner = executor.submit(
            cache.ensure_final_link_result, entry, first, produce_first
        )
        try:
            assert entered.wait(300)
            # A separate interpreter has no in-process lock registry: this is
            # an independent witness that the producer holds the OS key lock.
            probe = process_guard.run_completed_command(
                [
                    sys.executable,
                    "-c",
                    """
import hashlib
import os
import sys
from pathlib import Path
from molt.file_locks import _acquire_file_lock, _release_file_lock, FileLockTimeout
try:
    handle = _acquire_file_lock(Path(sys.argv[1]), timeout_s=0.1, timeout_message="held")
except FileLockTimeout:
    print("held")
else:
    _release_file_lock(handle)
    raise AssertionError("producer released its OS lock")
""",
                    str(entry.lock),
                ],
                capture_output=True,
                check=True,
                timeout=300,
            )
            assert probe.stdout == "held\n"
            competitor = executor.submit(waiter)
            assert waiter_started.wait(300)
            # Another key completes while this producer remains suspended.
            other = cache.final_link_cache_entry("8" * 64)
            unrelated = {"linked": tmp_path / "unrelated"}
            assert not cache.ensure_final_link_result(
                other, unrelated, lambda: write(unrelated)
            ).reused
            assert not release.is_set()
        finally:
            release.set()
        if first_fails:
            with pytest.raises(ValueError, match="first producer rejected"):
                owner.result(timeout=300)
        else:
            assert not owner.result(timeout=300).reused
        result = competitor.result(timeout=300)
    assert result.reused is not first_fails
    assert calls == (["first", "second"] if first_fails else ["first"])
    assert {role: path.read_bytes() for role, path in second.items()} == {
        role: (role + " admitted bytes").encode() for role in second
    }
    replay = {role: tmp_path / "replay" / role for role in second}
    assert cache.ensure_final_link_result(entry, replay, _miss).reused
    assert {role: path.read_bytes() for role, path in replay.items()} == {
        role: path.read_bytes() for role, path in second.items()
    }


def test_cache_publication_failure_keeps_admitted_private_product(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    entry = cache.final_link_cache_entry("9" * 64)
    output = tmp_path / "private"

    def produce() -> None:
        output.write_bytes(b"admitted")

    def refuse(_entry: wasm_link_cache.WasmLinkCacheEntry, _outputs: object) -> None:
        raise OSError("cache unavailable")

    monkeypatch.setattr(cache, "_publish_wasm_link_cache_bundle", refuse)
    result = cache.ensure_final_link_result(entry, {"linked": output}, produce)
    assert not result.reused and isinstance(result.publication_error, OSError)
    assert output.read_bytes() == b"admitted"
    assert not entry.metadata.exists()

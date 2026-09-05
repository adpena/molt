"""Every file/probe consumer uses the same handle/change-time authority."""

from __future__ import annotations

import hashlib
import os
import subprocess

import pytest

from molt import toolchain_identity as identity


@pytest.mark.parametrize(
    "consumer",
    [
        identity.stable_file_sha256,
        identity.stable_file_content_identity,
        identity.executable_content_identity,
        identity.native_executable_content_identity,
    ],
)
def test_content_consumers_reject_write_restore_during_hash(
    tmp_path, monkeypatch, consumer
):
    path = tmp_path / "tool.exe"
    data = b"MZ" + b"0" * 64
    path.write_bytes(data)
    before = path.stat()
    original = hashlib.file_digest

    def mutate(stream, algorithm):
        result = original(stream, algorithm)
        path.write_bytes(b"MZ" + b"1" * 64)
        path.write_bytes(data)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return result

    with monkeypatch.context() as scoped:
        scoped.setattr(hashlib, "file_digest", mutate)
        with pytest.raises(ValueError, match="changed"):
            consumer(path, label="fixture")


@pytest.mark.parametrize("mutate", [False, True])
def test_version_probe_hashes_once_and_closes_generation_fence(
    tmp_path, monkeypatch, mutate
):
    path = tmp_path / "tool.exe"
    data = b"MZ" + b"0" * 64
    path.write_bytes(data)
    calls = []
    capture = identity.stable_regular_file_identity

    def counted(path, **kwargs):
        calls.append(path)
        return capture(path, **kwargs)

    def run(argv, **kwargs):
        if mutate:
            before = path.stat()
            path.write_bytes(b"MZ" + b"1" * 64)
            path.write_bytes(data)
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return subprocess.CompletedProcess(argv, 0, "tool 1.2.3", "")

    monkeypatch.setattr(identity, "stable_regular_file_identity", counted)
    monkeypatch.setattr(identity.subprocess, "run", run)
    if mutate:
        with pytest.raises(ValueError, match="changed"):
            identity.probe_executable(
                path,
                version_arguments=[("--version",)],
                environment={},
                label="fixture",
            )
    else:
        result = identity.probe_executable(
            path, version_arguments=[("--version",)], environment={}, label="fixture"
        )
        assert result.sha256 == hashlib.sha256(data).hexdigest()
        assert result.version == "tool 1.2.3"
    assert calls == [path]

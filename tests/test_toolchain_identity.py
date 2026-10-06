"""Every file/probe consumer uses the same handle/change-time authority."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess
from contextlib import contextmanager
from dataclasses import replace
from types import SimpleNamespace

import pytest

from molt import toolchain_identity as identity


def test_capture_bytes_and_identity_share_one_stable_read(tmp_path, monkeypatch):
    path = tmp_path / "source.py"
    data = b"# coding: utf-8\nprint('captured')\n"
    path.write_bytes(data)
    opened_paths = []
    read_sizes = []
    original_open = identity.open_stable_regular_file

    class CountedStream:
        def __init__(self, stream):
            self.stream = stream

        def read(self, size=-1):
            read_sizes.append(size)
            return self.stream.read(size)

    @contextmanager
    def counted_open(path, **kwargs):
        opened_paths.append(path)
        with original_open(path, **kwargs) as opened:
            yield replace(opened, stream=CountedStream(opened.stream))

    monkeypatch.setattr(identity, "open_stable_regular_file", counted_open)
    captured, raw = identity.capture_stable_regular_file(path, label="fixture")
    assert raw == data
    assert captured.path == path
    assert captured.size == len(data)
    assert captured.sha256 == hashlib.sha256(data).hexdigest()
    assert opened_paths == [path]
    assert read_sizes == [len(data) + 1]
    identity.verify_stable_regular_file_identity(captured, label="fixture")
    assert read_sizes == [len(data) + 1]


@pytest.mark.skipif(os.name != "nt", reason="Windows mandatory file sharing")
@pytest.mark.parametrize("operation", ["overwrite", "delete"])
def test_stable_read_excludes_windows_writes_and_delete(tmp_path, operation):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    with pytest.raises(identity.StableRegularFileChangedError, match="conflicting"):
        with identity.open_stable_regular_file(path, label="payload") as opened:
            assert opened.stream.read() == b"original"
            if operation == "overwrite":
                path.write_bytes(b"modified")
            else:
                path.unlink()
    # The rejected operation never modified the bytes under the owned reader.
    assert path.read_bytes() == b"original"
    path.write_bytes(b"released")
    assert path.read_bytes() == b"released"


@pytest.mark.skipif(os.name != "nt", reason="Windows mandatory file sharing")
def test_stable_read_refuses_an_existing_windows_writer(tmp_path):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    with path.open("r+b"):
        with pytest.raises(identity.StableRegularFileChangedError, match="writer"):
            identity.stable_regular_file_identity(path, label="payload")
    assert identity.stable_regular_file_identity(path, label="payload").sha256 == (
        hashlib.sha256(b"original").hexdigest()
    )


def test_stable_read_allows_concurrent_readers(tmp_path):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    with identity.open_stable_regular_file(path, label="first") as first:
        with identity.open_stable_regular_file(path, label="second") as second:
            assert first.stream.read() == second.stream.read() == b"original"


@pytest.mark.skipif(os.name != "nt", reason="Windows mandatory file sharing")
@pytest.mark.parametrize("operation", ["overwrite", "delete"])
def test_stable_read_preserves_conflict_on_another_file(tmp_path, operation):
    source = tmp_path / "source"
    other = tmp_path / "other"
    source.write_bytes(b"source")
    other.write_bytes(b"other")
    with identity.open_stable_regular_file(other, label="other"):
        with pytest.raises(PermissionError) as caught:
            with identity.open_stable_regular_file(source, label="source") as opened:
                assert opened.stream.read() == b"source"
                if operation == "overwrite":
                    other.write_bytes(b"changed")
                else:
                    other.unlink()
        assert Path(caught.value.filename) == other
    assert source.read_bytes() == b"source"
    assert other.read_bytes() == b"other"


def test_mutation_version_avoids_hashing_and_detects_changed_size(
    tmp_path, monkeypatch
):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    before = path.stat()

    def forbid_hash(*args, **kwargs):
        pytest.fail("mutation-only capture must not read payload bytes")

    monkeypatch.setattr(identity, "_sha256_stream", forbid_hash)
    version = identity.stable_regular_file_version(path, label="payload")
    identity.verify_stable_regular_file_identity(version, label="payload")
    path.write_bytes(b"modified-content")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        identity.verify_stable_regular_file_identity(version, label="payload")


@pytest.mark.parametrize("operation", ["capture", "verify"])
def test_expected_path_stat_is_checked_without_repeating_the_opening_lookup(
    tmp_path, monkeypatch, operation
):
    path = tmp_path / "payload"
    path.write_bytes(b"captured")
    captured = identity.stable_regular_file_identity(path, label="payload")
    expected = path.lstat()
    lookups = []
    lstat = Path.lstat

    def counted(candidate, *args, **kwargs):
        if candidate == path:
            lookups.append(candidate)
        return lstat(candidate, *args, **kwargs)

    monkeypatch.setattr(Path, "lstat", counted)
    if operation == "capture":
        assert (
            identity.stable_regular_file_identity(
                path, label="payload", expected_path_stat=expected
            )
            == captured
        )
    else:
        identity.verify_stable_regular_file_identity(
            captured, label="payload", expected_path_stat=expected
        )
    # Both fresh opened-path and closing-path checks remain mandatory.
    assert lookups == [path, path]


@pytest.mark.parametrize("operation", ["capture", "verify"])
def test_expected_path_stat_rejects_a_different_snapshot_generation(
    tmp_path, operation
):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    expected = path.lstat()
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"original")
    os.utime(replacement, ns=(expected.st_atime_ns, expected.st_mtime_ns))
    os.replace(replacement, path)
    # The current generation is stable; only the supplied tree row is stale.
    current = identity.stable_regular_file_identity(path, label="payload")
    with pytest.raises(identity.StableRegularFileChangedError, match="changed"):
        if operation == "capture":
            identity.stable_regular_file_identity(
                path, label="payload", expected_path_stat=expected
            )
        else:
            identity.verify_stable_regular_file_identity(
                current, label="payload", expected_path_stat=expected
            )


def test_expected_path_stat_keeps_the_handle_mutation_fence(tmp_path, monkeypatch):
    path = tmp_path / "payload"
    path.write_bytes(b"original")
    expected = path.lstat()
    hash_stream = identity._sha256_stream

    def mutate_after_hash(stream):
        digest = hash_stream(stream)
        path.write_bytes(b"modified")
        os.utime(path, ns=(expected.st_atime_ns, expected.st_mtime_ns))
        return digest

    monkeypatch.setattr(identity, "_sha256_stream", mutate_after_hash)
    with pytest.raises(identity.StableRegularFileChangedError, match="changed"):
        identity.stable_regular_file_identity(
            path, label="payload", expected_path_stat=expected
        )


def test_capture_rejects_short_source_read(tmp_path, monkeypatch):
    path = tmp_path / "source.py"
    path.write_bytes(b"print('captured')\n")
    original_open = identity.open_stable_regular_file

    @contextmanager
    def incorrect_length(path, **kwargs):
        with original_open(path, **kwargs) as opened:
            yield replace(opened, stat=SimpleNamespace(st_size=opened.stat.st_size + 1))

    monkeypatch.setattr(identity, "open_stable_regular_file", incorrect_length)
    with pytest.raises(identity.StableRegularFileChangedError, match="size changed"):
        identity.capture_stable_regular_file(path, label="fixture")


@pytest.mark.parametrize("restore_content", [False, True])
def test_capture_rejects_write_and_restored_timestamp_during_read(
    tmp_path, monkeypatch, restore_content
):
    path = tmp_path / "source.py"
    data = b"print('original')\n"
    path.write_bytes(data)
    before = path.stat()
    original_hash = hashlib.sha256

    def mutate_after_read(raw):
        result = original_hash(raw)
        path.write_bytes(b"print('modified')\n")
        if restore_content:
            path.write_bytes(data)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return result

    monkeypatch.setattr(identity.hashlib, "sha256", mutate_after_read)
    with pytest.raises(identity.StableRegularFileChangedError, match="changed"):
        identity.capture_stable_regular_file(path, label="fixture")


def test_capture_identity_matches_streaming_and_snapshot_authorities(tmp_path):
    source = tmp_path / "source.py"
    source.write_bytes(b"print('same generation')\n")
    captured, raw = identity.capture_stable_regular_file(source, label="fixture")
    assert captured == identity.stable_regular_file_identity(source, label="fixture")
    snapshot = identity.snapshot_stable_regular_file(
        source, tmp_path / "snapshot.py", label="fixture"
    )
    assert snapshot.source == captured
    assert snapshot.snapshot.sha256 == captured.sha256
    assert raw == (tmp_path / "snapshot.py").read_bytes()


@pytest.mark.parametrize("consumer", ["command", "executable", "find"])
def test_user_tool_selectors_use_captured_home_not_ambient(
    tmp_path, monkeypatch, consumer
):
    name = "compiler.exe" if os.name == "nt" else "compiler"
    selected = tmp_path / "selected home"
    tool = selected / "tool directory" / name
    tool.parent.mkdir(parents=True)
    tool.write_bytes(b"selected compiler")
    tool.chmod(0o755)
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    environment = {home_key: str(selected), "PATHEXT": ".EXE"}
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    before = dict(os.environ)
    raw = f"~/tool directory/{name}"
    if consumer == "command":
        assert identity.resolve_explicit_tool_command(
            f'"{raw}" -c', label="compiler", environment=environment
        ) == (str(tool), "-c")
    elif consumer == "executable":
        assert (
            identity.resolve_executable(raw, label="compiler", environment=environment)
            == tool
        )
    else:
        assert identity.find_executable(raw, environment=environment) == tool
    assert dict(os.environ) == before


def test_default_user_path_expansion_matches_pathlib(tmp_path, monkeypatch):
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    monkeypatch.setenv(home_key, str(tmp_path))
    assert identity.expand_user_path("~/bin") == Path("~/bin").expanduser()


@pytest.mark.skipif(os.name != "nt", reason="Windows captured home contract")
def test_captured_windows_home_precedence_and_named_user(tmp_path, monkeypatch):
    parent = tmp_path / "Users"
    selected = parent / "selected"
    environment = {
        "UserProfile": str(selected),
        "UserName": "selected",
        "HomeDrive": "Z:",
        "HomePath": "\\unused",
        "HOME": "ignored",
    }
    monkeypatch.setenv("USERPROFILE", str(tmp_path / "ambient"))
    assert (
        identity.expand_user_path("~/bin", environment=environment) == selected / "bin"
    )
    assert (
        identity.expand_user_path("~selected/bin", environment=environment)
        == selected / "bin"
    )
    assert (
        identity.expand_user_path("~other/bin", environment=environment)
        == parent / "other" / "bin"
    )
    assert (
        identity.expand_user_path(
            "~/bin",
            environment={
                "HOMEDRIVE": selected.drive,
                "HOMEPATH": str(selected)[len(selected.drive) :],
            },
        )
        == selected / "bin"
    )
    with pytest.raises(ValueError, match="no user home"):
        identity.expand_user_path("~/bin", environment={})


@pytest.mark.skipif(os.name == "nt", reason="POSIX account database contract")
def test_captured_posix_home_and_named_account_have_separate_authorities(
    tmp_path, monkeypatch
):
    import pwd
    from types import SimpleNamespace

    monkeypatch.setenv("HOME", str(tmp_path / "ambient"))
    monkeypatch.setattr(
        pwd, "getpwuid", lambda _uid: SimpleNamespace(pw_dir=str(tmp_path / "system"))
    )
    monkeypatch.setattr(
        pwd, "getpwnam", lambda _name: SimpleNamespace(pw_dir=str(tmp_path / "named"))
    )
    environment = {"HOME": str(tmp_path / "selected")}
    assert (
        identity.expand_user_path("~/bin", environment=environment)
        == tmp_path / "selected" / "bin"
    )
    assert (
        identity.expand_user_path("~/bin", environment={})
        == tmp_path / "system" / "bin"
    )
    assert (
        identity.expand_user_path("~other/bin", environment=environment)
        == tmp_path / "named" / "bin"
    )


@pytest.mark.parametrize("relative", [False, True])
def test_tool_command_exact_path_with_spaces_uses_captured_cwd(
    tmp_path, monkeypatch, relative
):
    selected_cwd = tmp_path / "selected"
    tool = selected_cwd / "tool directory" / "compiler.exe"
    tool.parent.mkdir(parents=True)
    tool.write_bytes(b"selected compiler")
    ambient_cwd = tmp_path / "ambient"
    ambient_cwd.mkdir()
    monkeypatch.chdir(ambient_cwd)
    raw = str(tool.relative_to(selected_cwd)) if relative else str(tool)
    assert identity.resolve_explicit_tool_command(
        raw, label="compiler", environment={}, cwd=selected_cwd
    ) == (str(tool),)
    assert identity.resolve_explicit_tool_command(
        f'"{raw}" --target wasm32-wasip1',
        label="compiler",
        environment={},
        cwd=selected_cwd,
    ) == (str(tool), "--target", "wasm32-wasip1")


def test_tool_command_relative_search_roots_use_captured_not_ambient_cwd(
    tmp_path, monkeypatch
):
    selected_cwd = tmp_path / "selected"
    name = "compiler.exe" if os.name == "nt" else "compiler"
    tool = selected_cwd / "bin" / name
    tool.parent.mkdir(parents=True)
    tool.write_bytes(b"selected compiler")
    tool.chmod(0o755)
    ambient = tmp_path / "ambient"
    ambient.mkdir()
    monkeypatch.chdir(ambient)
    monkeypatch.setenv("PATH", str(ambient))
    command = identity.resolve_explicit_tool_command(
        name + " -c",
        label="compiler",
        cwd=selected_cwd,
        environment={"PATH": "bin", "PATHEXT": ".EXE"},
    )
    assert command == (str(tool), "-c")


@pytest.mark.skipif(os.name != "nt", reason="Windows executable name contract")
@pytest.mark.parametrize("consumer", ["search", "explicit", "identity"])
def test_executable_entrypoint_spelling_is_canonical_across_selectors(
    tmp_path, consumer
):
    tool = tmp_path / "clang-cl.exe"
    tool.write_bytes(b"selected compiler")
    environment = {"PATH": str(tmp_path), "PATHEXT": ".EXE"}
    if consumer == "search":
        selected = identity.find_executable("clang-cl", environment=environment)
    elif consumer == "explicit":
        selected = Path(
            identity.resolve_explicit_tool_command(
                str(tool.with_name("Clang-CL.EXE")),
                label="compiler",
                environment=environment,
            )[0]
        )
    else:
        selected = identity.resolve_executable(
            str(tool.with_suffix(".EXE")), label="compiler", environment=environment
        )
    assert selected is not None
    # Path equality on Windows would conceal the regression in argv spelling.
    assert selected.name == "clang-cl.exe"
    assert selected.samefile(tool)


def test_executable_entrypoint_keeps_lexical_alias(tmp_path):
    content = tmp_path / "driver.exe"
    content.write_bytes(b"one driver, multiple entrypoints")
    alias = tmp_path / "clang-cl.exe"
    try:
        alias.symlink_to(content)
    except OSError:
        pytest.skip("host cannot create executable symlinks")
    selected = identity.resolve_executable(
        str(alias.with_suffix(".EXE") if os.name == "nt" else alias),
        label="compiler",
        environment={},
    )
    assert selected.name == "clang-cl.exe"
    assert selected.is_symlink()
    assert identity.executable_content_path(selected, label="compiler") == content


@pytest.mark.parametrize("value", ["", '"', "bad\x00command"])
def test_tool_command_rejects_malformed_input(tmp_path, value):
    with pytest.raises(ValueError, match="compiler"):
        identity.resolve_explicit_tool_command(
            value, label="compiler", environment={}, cwd=tmp_path
        )


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
    original = identity._sha256_stream
    calls = []

    def mutate(stream):
        calls.append(stream)
        result = original(stream)
        path.write_bytes(b"MZ" + b"1" * 64)
        path.write_bytes(data)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return result

    with monkeypatch.context() as scoped:
        scoped.setattr(identity, "_sha256_stream", mutate)
        with pytest.raises(ValueError, match="changed"):
            consumer(path, label="fixture")
    assert len(calls) == 1


def test_executable_digest_and_header_share_one_owned_descriptor(tmp_path, monkeypatch):
    path = tmp_path / "tool.exe"
    data = b"MZ00payload"
    path.write_bytes(data)
    opened = []
    descriptor = identity.open_stable_read_descriptor

    def open_descriptor(path):
        opened.append(path)
        return descriptor(path)

    monkeypatch.setattr(identity, "open_stable_read_descriptor", open_descriptor)
    lexical, resolved, size, digest, header = identity._stable_file_content(
        path, label="fixture"
    )
    assert (lexical, resolved, size, digest, header) == (
        path,
        path,
        len(data),
        hashlib.sha256(data).hexdigest(),
        b"MZ00",
    )
    assert opened == [path]


@pytest.mark.parametrize("mutate", [False, True])
def test_version_probe_hashes_once_and_closes_generation_fence(
    tmp_path, monkeypatch, mutate
):
    path = tmp_path / "tool.exe"
    data = b"MZ" + b"0" * 64
    path.write_bytes(data)
    calls = []
    capture = identity.stable_regular_file_handle_identity

    def counted(opened, **kwargs):
        calls.append(opened.path)
        return capture(opened, **kwargs)

    def run(argv, **kwargs):
        if mutate:
            before = path.stat()
            path.write_bytes(data + b"changed-size")
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return subprocess.CompletedProcess(argv, 0, "tool 1.2.3", "")

    monkeypatch.setattr(identity, "stable_regular_file_handle_identity", counted)
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


@pytest.mark.parametrize("header", [b"MZ00", b"\x7fELF", b"\xcf\xfa\xed\xfe"])
def test_native_content_and_cargo_custody_share_one_hash_authority(
    tmp_path, monkeypatch, header
):
    from molt.cli.runtime_cargo_plan import CargoExecutableCustody

    path = tmp_path / "tool.exe"
    path.write_bytes(header + b"payload")
    capture = identity.stable_regular_file_handle_identity
    calls = []

    def counted(opened, **kwargs):
        calls.append(opened.path)
        return capture(opened, **kwargs)

    monkeypatch.setattr(identity, "stable_regular_file_handle_identity", counted)
    content = identity.native_executable_content_identity(path, label="fixture")
    assert calls == [path]
    calls.clear()
    custody = CargoExecutableCustody.capture("tool/final_linker", path)
    assert custody.content_record() == content
    custody.verify()
    assert calls == [path]


@pytest.mark.parametrize(
    "content", [b"#!/bin/sh\nexec tool\n", b"@echo off\ntool.exe\n"]
)
@pytest.mark.parametrize("consumer", ["content", "version", "cargo"])
def test_native_consumers_reject_scripts_before_execution(
    tmp_path, monkeypatch, content, consumer
):
    from molt.cli.runtime_cargo_plan import CargoExecutableCustody

    path = tmp_path / "wrapper"
    path.write_bytes(content)

    def unexpected_execution(*args, **kwargs):
        pytest.fail("script was executed before native admission")

    monkeypatch.setattr(identity.subprocess, "run", unexpected_execution)
    with pytest.raises(ValueError, match="native executable, not a script"):
        if consumer == "content":
            identity.native_executable_content_identity(path, label="fixture")
        elif consumer == "version":
            identity.probe_executable(
                path,
                version_arguments=[("--version",)],
                environment={},
                label="fixture",
            )
        else:
            CargoExecutableCustody.capture("tool/final_linker", path)


def test_generic_executable_probe_remains_script_capable(tmp_path):
    path = tmp_path / "script"
    data = b"#!/bin/sh\nexit 0\n"
    path.write_bytes(data)
    with identity.stable_executable_probe(path, label="script") as (
        entrypoint,
        captured,
    ):
        assert entrypoint == path
        assert captured.sha256 == hashlib.sha256(data).hexdigest()


def test_verified_executable_probe_reuses_and_fences_captured_generation(tmp_path):
    path = tmp_path / "llvm-nm"
    path.write_bytes(b"generation-a")
    with identity.stable_executable_probe(path, label="symbol reader") as (
        entrypoint,
        captured,
    ):
        pass

    with identity.stable_executable_probe(
        entrypoint, label="symbol reader", identity=captured
    ) as (warm_entrypoint, warm_identity):
        assert warm_entrypoint == entrypoint
        assert warm_identity == captured

    path.write_bytes(b"generation-b-longer")
    with pytest.raises(ValueError, match="changed since identity capture"):
        with identity.stable_executable_probe(
            entrypoint, label="symbol reader", identity=captured
        ):
            pass


@pytest.mark.parametrize("change", ["none", "rewrite", "replace", "symlink"])
def test_path_fence_names_only_the_captured_generation(tmp_path, change):
    path = tmp_path / "input.py"
    path.write_bytes(b"before")
    captured = identity.stable_regular_file_identity(path, label="input")
    before = path.lstat()
    if change == "rewrite":
        path.write_bytes(b"after-with-changed-size")
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif change == "replace":
        replacement = tmp_path / "replacement.py"
        replacement.write_bytes(b"before")
        os.utime(replacement, ns=(before.st_atime_ns, before.st_mtime_ns))
        os.replace(replacement, path)
    elif change == "symlink":
        other = tmp_path / "other.py"
        other.write_bytes(b"before")
        path.unlink()
        try:
            path.symlink_to(other)
        except OSError as exc:
            pytest.skip(f"symlink creation unavailable: {exc}")
    assert identity.stable_regular_file_path_is_current(captured, path.lstat()) is (
        change == "none"
    )


def test_snapshot_discard_never_unlinks_a_replacement(tmp_path):
    source = tmp_path / "source"
    source.write_bytes(b"payload")
    owned = identity.snapshot_stable_regular_file(
        source, tmp_path / "owned", label="payload"
    )
    owned.discard()
    assert not (tmp_path / "owned").exists()
    replaced = identity.snapshot_stable_regular_file(
        source, tmp_path / "replaced", label="payload"
    )
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"payload")
    os.replace(replacement, tmp_path / "replaced")
    replaced.discard()
    assert (tmp_path / "replaced").read_bytes() == b"payload"


@pytest.mark.parametrize("data", [b"#define VALUE 42\n", b"--export=example\n"])
def test_resource_custody_does_not_claim_native_executable_admission(tmp_path, data):
    from molt.cli.runtime_cargo_plan import (
        CargoExecutableCustody,
        CargoFileCustody,
        CargoResourceCustody,
        CargoResourceRoot,
    )

    path = tmp_path / "resource"
    path.write_bytes(data)
    resources = CargoResourceCustody.capture((CargoResourceRoot("input", path),))
    assert len(resources.files) == 1
    captured = resources.files[0]
    assert type(captured) is CargoFileCustody
    assert captured.identity.sha256 == hashlib.sha256(data).hexdigest()
    resources.verify()
    with pytest.raises(ValueError, match="native executable, not a script"):
        CargoExecutableCustody.capture("tool/final_linker", path)


@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
def test_observed_reader_rejects_a_new_generation_before_yield(tmp_path, mutation):
    path = tmp_path / "payload"
    path.write_bytes(b"before")
    observed = identity.stable_regular_file_identity(path, label="fixture")
    metadata = path.stat()
    if mutation == "replace":
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"before")
        replacement.replace(path)
    else:
        path.write_bytes(b"after-with-changed-size")
    os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    with pytest.raises(ValueError, match="changed since identity capture"):
        with identity.open_stable_regular_file(
            path, label="fixture", observed=observed
        ):
            pytest.fail("changed generation escaped its opened-handle fence")


@pytest.mark.parametrize("mutate", [False, True])
def test_attested_read_checks_content_with_one_owned_read(
    tmp_path, monkeypatch, mutate
):
    path = tmp_path / "payload"
    path.write_bytes(b"before")
    captured = identity.stable_regular_file_identity(path, label="fixture")
    if mutate:
        before = path.stat()
        path.write_bytes(b"after!")
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        current = identity.stable_regular_file_version(path, label="current metadata")
        # Model the allowed equal-metadata state deterministically. The digest
        # still identifies the original bytes, so only content validation can
        # reject this same-size replacement; no clock tick is assumed unique.
        captured = replace(
            captured,
            _stat_identity=current._stat_identity,
            _content_change_time_ns=current._content_change_time_ns,
        )
    opens = []
    reads = []
    open_stable = identity.open_stable_regular_file

    class CountedStream:
        def __init__(self, stream):
            self.stream = stream

        def read(self, size=-1):
            reads.append(size)
            return self.stream.read(size)

    @contextmanager
    def counted_open(path, **kwargs):
        opens.append(path)
        with open_stable(path, **kwargs) as opened:
            yield replace(opened, stream=CountedStream(opened.stream))

    monkeypatch.setattr(identity, "open_stable_regular_file", counted_open)
    if mutate:
        with pytest.raises(
            identity.StableRegularFileChangedError, match="content changed"
        ):
            identity.read_stable_regular_file(captured, label="fixture")
    else:
        assert identity.read_stable_regular_file(captured, label="fixture") == b"before"
    assert opens == [path]
    assert reads == [captured.size + 1]


def test_bounded_capture_never_issues_an_unbounded_read(tmp_path, monkeypatch):
    path = tmp_path / "metadata"
    path.write_bytes(b"small")
    original = identity.open_stable_regular_file
    reads = []

    class GrowingStream:
        def read(self, size=-1):
            reads.append(size)
            assert size == 6
            return b"x" * size

    @contextmanager
    def growing_open(path, **kwargs):
        with original(path, **kwargs) as opened:
            yield replace(opened, stream=GrowingStream())

    monkeypatch.setattr(identity, "open_stable_regular_file", growing_open)
    with pytest.raises(ValueError, match="size changed"):
        identity.capture_stable_regular_file(path, label="fixture", max_bytes=8)
    assert reads == [6]


def test_attested_reader_rejects_declared_size_above_requested_limit(tmp_path):
    path = tmp_path / "metadata"
    path.write_bytes(b"five!")
    captured = identity.stable_regular_file_identity(path, label="fixture")
    with pytest.raises(ValueError, match="exceeds size limit"):
        identity.read_stable_regular_file(captured, label="fixture", max_bytes=4)

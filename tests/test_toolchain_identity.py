"""Every file/probe consumer uses the same handle/change-time authority."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess

import pytest

from molt import toolchain_identity as identity


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


@pytest.mark.parametrize("header", [b"MZ00", b"\x7fELF", b"\xcf\xfa\xed\xfe"])
def test_native_content_and_cargo_custody_share_one_hash_authority(
    tmp_path, monkeypatch, header
):
    from molt.cli.runtime_cargo_plan import CargoExecutableCustody

    path = tmp_path / "tool.exe"
    path.write_bytes(header + b"payload")
    capture = identity.stable_regular_file_identity
    calls = []

    def counted(path, **kwargs):
        calls.append(path)
        return capture(path, **kwargs)

    monkeypatch.setattr(identity, "stable_regular_file_identity", counted)
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

    path.write_bytes(b"generation-b")
    with pytest.raises(ValueError, match="changed since identity capture"):
        with identity.stable_executable_probe(
            entrypoint, label="symbol reader", identity=captured
        ):
            pass


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

"""Teeth for the content-addressed native proof supervisor cache."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import supervisor_custody as sc

RUSTC_OUTPUT = (
    "rustc 1.96.1 (abcdef123 2026-06-01)\nbinary: rustc\ncommit-hash: abcdef123\n"
    "commit-date: 2026-06-01\nhost: x86_64-pc-windows-msvc\nrelease: 1.96.1\n"
)


def _completed(stdout: str, returncode: int = 0) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        args=[], returncode=returncode, stdout=stdout, stderr=""
    )


@pytest.fixture
def cache(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    source_root = tmp_path / "supervisor-src"
    (source_root / "src").mkdir(parents=True)
    (source_root / "Cargo.toml").write_text(
        '[package]\nname = "sup"\n', encoding="utf-8"
    )
    (source_root / "Cargo.lock").write_text("# lock\n", encoding="utf-8")
    (source_root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
    monkeypatch.setattr(sc, "SUPERVISOR_SOURCE_ROOT", source_root)
    cache_root = tmp_path / "custody" / "proof-supervisor"
    monkeypatch.setattr(sc, "supervisor_cache_root", lambda env, **_k: cache_root)
    built = tmp_path / "target" / "release" / sc.SUPERVISOR_BINARY_NAME
    built.parent.mkdir(parents=True)
    built.write_bytes(b"supervisor-bytes-v1")
    builds: list[tuple[str, ...]] = []

    def fake_run(command, *, cwd, env, timeout=30.0, text=True):  # type: ignore[no-untyped-def]
        del cwd, env, timeout, text
        command = tuple(str(part) for part in command)
        if command[:2] == ("rustc", "-vV"):
            return _completed(RUSTC_OUTPUT)
        builds.append(command)
        return _completed(f"building...\n{built}\n")

    monkeypatch.setattr(command_identity, "_run_captured", fake_run)
    return {
        "root": source_root,
        "cache_root": cache_root,
        "built": built,
        "builds": builds,
        "cwd": tmp_path / "repo",
        "env": {"CARGO_TARGET_DIR": str(tmp_path / "target")},
    }


def test_identity_is_content_addressed(cache: dict[str, Any]) -> None:
    first = sc.supervisor_source_identity(cache["env"])
    assert first["schema"] == sc.SUPERVISOR_IDENTITY_SCHEMA
    assert [row["path"] for row in first["sources"]] == [
        "Cargo.toml",
        "Cargo.lock",
        "src/main.rs",
    ]
    assert first["rustc"]["release"] == "1.96.1"
    assert sc.supervisor_source_identity(cache["env"])["identity"] == first["identity"]
    (cache["root"] / "src" / "main.rs").write_text(
        "fn main() { let _x = 1; }\n", encoding="utf-8"
    )
    assert sc.supervisor_source_identity(cache["env"])["identity"] != first["identity"]


def test_miss_builds_once_then_hits(cache: dict[str, Any]) -> None:
    (cache["cwd"]).mkdir()
    binary, telemetry = sc._provision_proof_supervisor(
        cwd=cache["cwd"], env=cache["env"]
    )
    assert telemetry["cache"] == "miss"
    assert len(cache["builds"]) == 1
    assert binary.read_bytes() == b"supervisor-bytes-v1"
    assert binary.parent.parent == cache["cache_root"]
    manifest = json.loads((binary.parent / "identity.json").read_text(encoding="utf-8"))
    assert (
        manifest["binary_sha256"] == command_identity._file_identity(binary)["sha256"]
    )
    assert manifest["identity"] == telemetry["source_identity"]

    again, telemetry = sc._provision_proof_supervisor(
        cwd=cache["cwd"], env=cache["env"]
    )
    assert telemetry["cache"] == "hit"
    assert again == binary
    assert len(cache["builds"]) == 1


def test_corrupted_cache_entry_is_refused(cache: dict[str, Any]) -> None:
    cache["cwd"].mkdir()
    binary, _ = sc._provision_proof_supervisor(cwd=cache["cwd"], env=cache["env"])
    binary.write_bytes(b"tampered")
    assert sc._cached_supervisor(binary.parent) is None
    # A tampered entry occupies the identity directory; provisioning must not
    # silently adopt it.
    with pytest.raises(ValueError, match="failed verification"):
        sc._provision_proof_supervisor(cwd=cache["cwd"], env=cache["env"])
    assert len(cache["builds"]) == 2


def test_source_change_yields_a_new_entry(cache: dict[str, Any]) -> None:
    cache["cwd"].mkdir()
    first, _ = sc._provision_proof_supervisor(cwd=cache["cwd"], env=cache["env"])
    (cache["root"] / "src" / "main.rs").write_text(
        "fn main() { let _y = 2; }\n", encoding="utf-8"
    )
    cache["built"].write_bytes(b"supervisor-bytes-v2")
    second, telemetry = sc._provision_proof_supervisor(
        cwd=cache["cwd"], env=cache["env"]
    )
    assert telemetry["cache"] == "miss"
    assert second != first
    assert second.read_bytes() == b"supervisor-bytes-v2"
    assert first.read_bytes() == b"supervisor-bytes-v1"


def test_scratch_source_roots_never_cache_under_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "scratch-repo"
    source.mkdir()

    class Custody:
        custody_root = source

    monkeypatch.setattr(sc, "checkout_custody", lambda *_a, **_k: Custody())
    root = sc.supervisor_cache_root({}, source_root=source)
    assert not root.is_relative_to(source)
    assert root.name == sc.SUPERVISOR_CACHE_DIRNAME


def test_cache_root_follows_the_queue_checkout_not_the_proof_project(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Custody:
        custody_root = tmp_path / "Molt"

    seen: list[Path] = []

    def custody(source_root, *_a, **_k):
        seen.append(Path(source_root))
        return Custody()

    monkeypatch.setattr(sc, "checkout_custody", custody)
    root = sc.supervisor_cache_root({})
    assert seen == [sc.admission._REPO_ROOT.resolve()]
    assert root == tmp_path / "Molt" / sc.SUPERVISOR_CACHE_DIRNAME


def test_durable_custody_root_hosts_the_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "Molt" / "molt-src"
    source.mkdir(parents=True)

    class Custody:
        custody_root = tmp_path / "Molt"

    monkeypatch.setattr(sc, "checkout_custody", lambda *_a, **_k: Custody())
    assert (
        sc.supervisor_cache_root({}, source_root=source)
        == tmp_path / "Molt" / sc.SUPERVISOR_CACHE_DIRNAME
    )

from __future__ import annotations

import hashlib
import os
from pathlib import Path

from molt.cli import backend_cache
from molt.cli import llvm_wasi_tools
from molt.cli import source_extension_toolchain
import pytest


_TOOL_FILE_NAMES = {
    "cc": "clang",
    "cxx": "clang++",
    "wasm_ld": "wasm-ld",
    "ar": "llvm-ar",
    "ranlib": "llvm-ranlib",
    "nm": "llvm-nm",
    "strip": "llvm-strip",
}


def _tool_path(directory: Path, role: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return directory / f"{_TOOL_FILE_NAMES[role]}{suffix}"


def _write_tool_family(directory: Path) -> dict[str, Path]:
    directory.mkdir(parents=True)
    paths: dict[str, Path] = {}
    for role in _TOOL_FILE_NAMES:
        path = _tool_path(directory, role)
        path.write_bytes(b"tool")
        paths[role] = path
    return paths


def test_tool_family_resolves_every_tool_from_explicit_compiler_siblings(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = _write_tool_family(tmp_path / "LLVM" / "bin")
    monkeypatch.setattr(
        llvm_wasi_tools,
        "_tool_version",
        lambda path: f"version:{path.name}",
    )
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        explicit_commands={"cc": (str(paths["cc"]), "--sysroot", "sdk")}
    )

    assert family.missing_roles() == ()
    assert family.cc is not None
    assert family.cc.command == (str(paths["cc"]), "--sysroot", "sdk")
    assert family.metadata()["nm"] == {
        "command": [str(paths["nm"].resolve())],
        "path": str(paths["nm"].resolve()),
        "sha256": hashlib.sha256(b"tool").hexdigest(),
        "version": f"version:{paths['nm'].name}",
    }


def test_tool_family_resolves_managed_target_root_before_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    managed = _write_tool_family(tmp_path / "target" / "toolchains" / "llvm-22" / "bin")
    monkeypatch.setattr(llvm_wasi_tools, "_tool_version", lambda _path: "22.1.8")
    monkeypatch.setattr(
        llvm_wasi_tools.shutil,
        "which",
        lambda name: f"/path/{name}",
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_root=tmp_path / "target"
    )

    assert family.missing_roles() == ()
    assert family.cc is not None
    assert family.cc.path == managed["cc"].resolve()
    assert family.nm is not None
    assert family.nm.path == managed["nm"].resolve()


def test_source_commands_share_family_and_never_duplicate_target() -> None:
    def tool(
        role: llvm_wasi_tools.LlvmToolRole,
        command: tuple[str, ...],
    ) -> llvm_wasi_tools.ResolvedLlvmTool:
        return llvm_wasi_tools.ResolvedLlvmTool(
            role=role,
            command=command,
            path=Path(command[0]),
            version="22.1.8",
            sha256="a" * 64,
        )

    family = llvm_wasi_tools.LlvmWasiToolFamily(
        cc=tool("cc", ("clang", "--target=wasm32-wasip1", "--sysroot", "sdk")),
        cxx=tool("cxx", ("clang++", "-target", "wasm32-wasip1", "--sysroot", "sdk")),
        wasm_ld=tool("wasm_ld", ("wasm-ld",)),
        ar=tool("ar", ("llvm-ar",)),
        ranlib=tool("ranlib", ("llvm-ranlib",)),
        nm=tool("nm", ("llvm-nm",)),
        strip=tool("strip", ("llvm-strip",)),
    )
    toolchain = source_extension_toolchain._SourceExtensionWasmToolchain(
        ok=True,
        compiler_kind="clang",
        tools=family,
        wasi_sysroot=Path("sdk"),
        detail="complete",
    )

    commands = source_extension_toolchain._source_extension_c_commands(
        toolchain=toolchain,
        target_triple="wasm32-wasip1",
    )

    assert commands["c"].count("--target=wasm32-wasip1") == 1
    assert commands["cpp"].count("-target") == 1
    assert commands["nm"] == ("llvm-nm",)
    assert commands["ranlib"] == ("llvm-ranlib",)
    assert commands["ld"] == ("wasm-ld",)


def test_backend_symbol_reader_consumes_canonical_nm_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: list[tuple[str, bool]] = []

    def candidates(
        role: llvm_wasi_tools.LlvmToolRole,
        *,
        include_rust_toolchain: bool,
    ) -> tuple[Path, ...]:
        seen.append((role, include_rust_toolchain))
        return (Path("/llvm/bin/llvm-nm"), Path("/usr/bin/nm"))

    monkeypatch.setattr(backend_cache, "llvm_tool_candidates", candidates)

    assert backend_cache._nm_candidate_binaries() == [
        str(Path("/llvm/bin/llvm-nm")),
        str(Path("/usr/bin/nm")),
    ]
    assert seen == [("nm", True)]

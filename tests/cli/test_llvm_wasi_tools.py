from __future__ import annotations

import hashlib
import os
from pathlib import Path

from molt.cli import backend_cache
from molt.cli import llvm_wasi_tools
from molt.cli import source_extension_target
from molt.cli import source_extension_toolchain
from molt.llvm_linker_roles import LlvmLinkerRole, executable_selects_linker_role
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


@pytest.fixture(autouse=True)
def _isolate_tool_candidate_cache():
    llvm_wasi_tools.clear_llvm_tool_candidate_cache()
    yield
    llvm_wasi_tools.clear_llvm_tool_candidate_cache()


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


def _replace_wasm_ld_with_driver_alias(paths: dict[str, Path]) -> tuple[Path, Path]:
    alias = paths["wasm_ld"]
    alias.unlink()
    suffix = ".exe" if os.name == "nt" else ""
    driver = alias.parent / f"lld{suffix}"
    driver.write_bytes(b"generic lld driver")
    try:
        alias.symlink_to(driver.name)
    except OSError:
        # Windows hosts without symlink privilege still exercise the lexical
        # role identity through a second hardlink to the shared driver bytes.
        os.link(driver, alias)
    return alias, driver


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
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root: ()
    )

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


def test_wasm_ld_symlink_keeps_role_entrypoint_in_explicit_prefix(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = _write_tool_family(tmp_path / "LLVM" / "bin")
    alias, driver = _replace_wasm_ld_with_driver_alias(paths)
    monkeypatch.setattr(llvm_wasi_tools, "_tool_version", lambda _path: "22.1.8")
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root: ()
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        explicit_commands={"cc": (str(paths["cc"]),)}
    )

    assert family.wasm_ld is not None
    assert family.wasm_ld.path == alias.absolute()
    assert family.wasm_ld.command == (str(alias.absolute()),)
    assert family.wasm_ld.path != driver.absolute()


def test_wasm_ld_role_rejects_explicit_generic_driver_and_uses_named_sibling(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    directory = tmp_path / "llvm" / "bin"
    paths = _write_tool_family(directory)
    alias, driver = _replace_wasm_ld_with_driver_alias(paths)
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        explicit_commands={"wasm_ld": (str(driver),)},
        sibling_directories=(directory,),
    )

    assert family.wasm_ld is not None
    assert family.wasm_ld.path == alias.absolute()
    assert family.wasm_ld.command == (str(alias.absolute()),)


def test_wasm_ld_path_alias_remains_role_specific_across_cache_hits(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    directory = tmp_path / "path-bin"
    paths = _write_tool_family(directory)
    alias, driver = _replace_wasm_ld_with_driver_alias(paths)
    monkeypatch.setattr(
        llvm_wasi_tools,
        "_managed_llvm_bin_directories",
        lambda _target_root: (),
    )
    monkeypatch.setattr(
        llvm_wasi_tools.shutil,
        "which",
        lambda name: str(alias) if name == "wasm-ld" else None,
    )

    first = llvm_wasi_tools.llvm_tool_candidates("wasm_ld")
    second = llvm_wasi_tools.llvm_tool_candidates("wasm_ld")

    assert first == second == (alias.absolute(),)
    assert first[0] != driver.absolute()
    assert llvm_wasi_tools.llvm_tool_candidate_cache_info()["hits"] == 1


@pytest.mark.parametrize(
    ("path", "expected"),
    [
        (Path("/usr/lib/llvm-22/bin/wasm-ld"), True),
        (Path("/usr/lib/llvm-22/bin/lld"), False),
        (Path(r"C:\LLVM\bin\wasm-ld.exe"), True),
        (Path(r"C:\LLVM\bin\lld.exe"), False),
    ],
)
def test_wasm_ld_role_name_is_host_separator_independent(
    path: Path,
    expected: bool,
) -> None:
    assert llvm_wasi_tools._is_wasm_ld_entrypoint(path) is expected


@pytest.mark.parametrize(
    "role",
    ("wasm-ld", "ld.lld", "ld64.lld", "lld-link"),
)
def test_every_linker_role_preserves_its_alias_and_rejects_generic_driver(
    role: LlvmLinkerRole,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    directory = tmp_path / role.replace(".", "_") / "bin"
    directory.mkdir(parents=True)
    suffix = ".exe" if os.name == "nt" else ""
    driver = directory / f"lld{suffix}"
    driver.write_bytes(b"shared generic driver")
    alias = directory / f"{role}{suffix}"
    try:
        alias.symlink_to(driver.name)
    except OSError:
        os.link(driver, alias)
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root: ()
    )

    candidates = llvm_wasi_tools.llvm_linker_candidates(
        role,
        explicit_commands=((str(driver),),),
        sibling_directories=(directory,),
    )

    assert candidates == (alias.absolute(),)
    assert candidates[0] != driver.absolute()
    assert executable_selects_linker_role(candidates[0], role)


@pytest.mark.parametrize(
    ("requested", "wrong"),
    (
        ("wasm-ld", "ld.lld"),
        ("ld.lld", "ld64.lld"),
        ("ld64.lld", "lld-link"),
        ("lld-link", "wasm-ld"),
    ),
)
def test_linker_roles_never_accept_a_sibling_role(
    requested: LlvmLinkerRole,
    wrong: LlvmLinkerRole,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    wrong_path = tmp_path / wrong
    wrong_path.write_bytes(b"wrong role")
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root: ()
    )

    assert (
        llvm_wasi_tools.llvm_linker_candidates(
            requested,
            explicit_commands=((str(wrong_path),),),
        )
        == ()
    )


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


def test_worktree_resolver_reuses_common_checkout_managed_toolchain(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "canonical"
    worktree = tmp_path / "worktree"
    git_dir = canonical / ".git" / "worktrees" / "lane"
    git_dir.mkdir(parents=True)
    (worktree / "src" / "molt" / "cli").mkdir(parents=True)
    (worktree / ".git").write_text(f"gitdir: {git_dir}\n", encoding="utf-8")
    managed = _write_tool_family(
        canonical / "target" / "toolchains" / "llvm-22.1.8" / "bin"
    )
    monkeypatch.setattr(
        llvm_wasi_tools,
        "__file__",
        str(worktree / "src" / "molt" / "cli" / "llvm_wasi_tools.py"),
    )
    monkeypatch.delenv("MOLT_TARGET_ROOT", raising=False)
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)

    assert llvm_wasi_tools.llvm_tool_candidates("cc")[0] == managed["cc"].resolve()


def test_candidate_resolution_memoizes_filesystem_candidate_probes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    clang = _tool_path(tmp_path / "path-bin", "cc")
    clang.parent.mkdir()
    clang.write_bytes(b"clang")
    calls: list[str] = []
    managed_calls = 0

    def managed(_target_root: Path | None) -> tuple[Path, ...]:
        nonlocal managed_calls
        managed_calls += 1
        return ()

    def which(name: str) -> str | None:
        calls.append(name)
        return str(clang) if name == "clang" else None

    monkeypatch.setattr(llvm_wasi_tools, "_managed_llvm_bin_directories", managed)
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", which)

    first = llvm_wasi_tools.llvm_tool_candidates("cc")
    second = llvm_wasi_tools.llvm_tool_candidates("cc")

    assert first == second == (clang.resolve(),)
    assert calls == ["clang"]
    # The cheap search-topology snapshot is refreshed to detect installation;
    # candidate-name probes and PATH lookup remain memoized.
    assert managed_calls == 2
    assert llvm_wasi_tools.llvm_tool_candidate_cache_info() == {
        "hits": 1,
        "misses": 1,
        "maxsize": 256,
        "currsize": 1,
    }


def test_candidate_cache_keys_path_and_directory_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path_a = tmp_path / "path-a" / "clang.exe"
    path_b = tmp_path / "path-b" / "clang.exe"
    managed_dir = tmp_path / "managed"
    for path in (path_a, path_b):
        path.parent.mkdir()
        path.write_bytes(path.name.encode())

    monkeypatch.setattr(
        llvm_wasi_tools,
        "_managed_llvm_bin_directories",
        lambda _target_root: (managed_dir,),
    )

    def which(name: str) -> str | None:
        assert name == "clang"
        return str(path_a if os.environ["PATH"] == "A" else path_b)

    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", which)
    monkeypatch.setenv("PATH", "A")
    assert llvm_wasi_tools.llvm_tool_candidates("cc") == (path_a.resolve(),)
    monkeypatch.setenv("PATH", "B")
    assert llvm_wasi_tools.llvm_tool_candidates("cc") == (path_b.resolve(),)

    managed_dir.mkdir()
    managed = _tool_path(managed_dir, "cc")
    managed.write_bytes(b"managed")
    assert llvm_wasi_tools.llvm_tool_candidates("cc")[0] == managed.resolve()


def test_candidate_cache_revalidates_selected_path_removal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    managed_dir = tmp_path / "managed"
    managed_dir.mkdir()
    managed = _tool_path(managed_dir, "cc")
    fallback = _tool_path(tmp_path / "fallback", "cc")
    fallback.parent.mkdir()
    managed.write_bytes(b"managed")
    fallback.write_bytes(b"fallback")
    monkeypatch.setattr(
        llvm_wasi_tools,
        "_managed_llvm_bin_directories",
        lambda _target_root: (managed_dir,),
    )
    monkeypatch.setattr(
        llvm_wasi_tools.shutil,
        "which",
        lambda name: str(fallback) if name == "clang" else None,
    )

    assert llvm_wasi_tools.llvm_tool_candidates("cc")[0] == managed.resolve()
    managed.unlink()
    assert llvm_wasi_tools.llvm_tool_candidates("cc") == (fallback.resolve(),)


def test_candidate_cache_drops_removed_explicit_tool(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    explicit = _tool_path(tmp_path / "explicit", "cc")
    fallback = _tool_path(tmp_path / "fallback", "cc")
    explicit.parent.mkdir()
    fallback.parent.mkdir()
    explicit.write_bytes(b"explicit")
    fallback.write_bytes(b"fallback")
    monkeypatch.setattr(
        llvm_wasi_tools,
        "_managed_llvm_bin_directories",
        lambda _target_root: (),
    )
    monkeypatch.setattr(
        llvm_wasi_tools.shutil,
        "which",
        lambda name: str(fallback) if name == "clang" else None,
    )

    command = (str(explicit),)
    assert llvm_wasi_tools.llvm_tool_candidates("cc", explicit_commands=(command,)) == (
        explicit.resolve(),
        fallback.resolve(),
    )
    explicit.unlink()
    assert llvm_wasi_tools.llvm_tool_candidates("cc", explicit_commands=(command,)) == (
        fallback.resolve(),
    )


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

    target_plan = source_extension_target.resolve_source_extension_target_plan(
        "wasm",
        host_target_triple="x86_64-unknown-linux-gnu",
        host_platform="linux",
        host_arch="x86_64",
    )
    commands = source_extension_toolchain._source_extension_c_commands(
        toolchain=toolchain,
        target_plan=target_plan,
    )

    assert set(commands) == {"ar", "c", "cpp", "ld", "nm", "ranlib", "strip"}
    assert commands["c"].count("--target=wasm32-wasip1") == 1
    assert commands["cpp"].count("-target") == 1
    assert commands["nm"] == ("llvm-nm",)
    assert commands["ranlib"] == ("llvm-ranlib",)
    assert commands["ld"] == ("wasm-ld",)

    with pytest.raises(ValueError, match="target conflicts"):
        source_extension_toolchain._compiler_command_with_target(
            ("clang", "--target=wasm32-unknown-unknown"),
            target_plan.target_triple,
        )


def test_explicit_wasm_compiler_preserves_validated_sysroot_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sysroot = (tmp_path / "wasi-sysroot").resolve()
    sysroot.mkdir()
    compiler = (
        "/tools/clang",
        "--target=wasm32-wasip1",
        "--sysroot",
        str(sysroot),
    )

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

    def family(
        *,
        explicit_commands: dict[llvm_wasi_tools.LlvmToolRole, tuple[str, ...]],
    ) -> llvm_wasi_tools.LlvmWasiToolFamily:
        return llvm_wasi_tools.LlvmWasiToolFamily(
            cc=tool("cc", explicit_commands["cc"]),
            cxx=tool("cxx", ("/tools/clang++",)),
            wasm_ld=tool("wasm_ld", ("/tools/wasm-ld",)),
            ar=tool("ar", ("/tools/llvm-ar",)),
            ranlib=tool("ranlib", ("/tools/llvm-ranlib",)),
            nm=tool("nm", ("/tools/llvm-nm",)),
            strip=tool("strip", ("/tools/llvm-strip",)),
        )

    monkeypatch.setattr(
        source_extension_toolchain,
        "resolve_explicit_tool_command",
        lambda _raw, *, label: compiler,
    )
    monkeypatch.setattr(
        source_extension_toolchain,
        "normalize_wasi_sysroot",
        lambda raw: Path(raw).resolve(),
    )
    monkeypatch.setattr(
        source_extension_toolchain,
        "resolve_llvm_wasi_tool_family",
        family,
    )
    monkeypatch.setattr(
        source_extension_toolchain,
        "_probe_wasm_source_extension_compiler",
        lambda _command, *, target_plan: None,
    )
    target_plan = source_extension_target.resolve_source_extension_target_plan(
        "wasm",
        host_target_triple="x86_64-unknown-linux-gnu",
        host_platform="linux",
        host_arch="x86_64",
    )

    resolved = source_extension_toolchain._resolve_env_wasm_compiler(
        env_name="MOLT_WASM_CC",
        raw_command="configured-clang",
        target_plan=target_plan,
    )

    assert resolved.ok is True
    assert resolved.wasi_sysroot == sysroot
    assert resolved.tools.cc is not None
    assert resolved.tools.cc.command == compiler


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

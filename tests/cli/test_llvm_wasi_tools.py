from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess

from molt.cli import native_symbol_inspection
from molt.cli import llvm_wasi_tools
from molt.cli import source_extension_target
from molt.cli import source_extension_toolchain
from molt import llvm_toolchain
from molt.toolchain_identity import resolve_explicit_tool_command
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
        lambda path, **_kwargs: f"version:{path.name}",
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root, **_kwargs: ()
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="wasm",
        explicit_commands={"cc": (str(paths["cc"]), "--sysroot", "sdk")},
    )

    assert family.missing_roles() == ()
    assert family.cc is not None
    assert family.cc.command == (str(paths["cc"]), "--sysroot", "sdk")
    assert family.nm is not None
    assert family.nm.path == paths["nm"].absolute()
    assert family.nm.command == (str(family.nm.path),)
    assert family.metadata()["nm"] == {
        "command": [str(family.nm.path)],
        "path": str(family.nm.path),
        "sha256": hashlib.sha256(b"tool").hexdigest(),
        "version": f"version:{family.nm.path.name}",
    }


def test_wasm_ld_symlink_keeps_role_entrypoint_in_explicit_prefix(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = _write_tool_family(tmp_path / "LLVM" / "bin")
    alias, driver = _replace_wasm_ld_with_driver_alias(paths)
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **_kwargs: "22.1.8"
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root, **_kwargs: ()
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="wasm", explicit_commands={"cc": (str(paths["cc"]),)}
    )

    assert family.wasm_ld is not None
    assert family.wasm_ld.path == alias.absolute()
    assert family.wasm_ld.command == (str(family.wasm_ld.path),)
    assert family.wasm_ld.path != driver.absolute()
    assert executable_selects_linker_role(family.wasm_ld.path, "wasm-ld")


def test_wasm_ld_role_rejects_explicit_generic_driver_and_uses_named_sibling(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    directory = tmp_path / "llvm" / "bin"
    paths = _write_tool_family(directory)
    alias, driver = _replace_wasm_ld_with_driver_alias(paths)
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="wasm",
        explicit_commands={"wasm_ld": (str(driver),)},
        sibling_directories=(directory,),
    )

    assert family.wasm_ld is not None
    assert family.wasm_ld.path == alias.absolute()
    assert family.wasm_ld.command == (str(family.wasm_ld.path),)
    assert family.wasm_ld.path != driver.absolute()
    assert executable_selects_linker_role(family.wasm_ld.path, "wasm-ld")


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
        lambda _target_root, **_kwargs: (),
    )
    monkeypatch.setattr(
        llvm_wasi_tools,
        "find_executable",
        lambda name, **_kwargs: str(alias) if name == "wasm-ld" else None,
    )

    first = llvm_wasi_tools.llvm_tool_candidates("wasm_ld", target_family="wasm")
    second = llvm_wasi_tools.llvm_tool_candidates("wasm_ld", target_family="wasm")

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
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root, **_kwargs: ()
    )

    candidates = llvm_wasi_tools.llvm_linker_candidates(
        role,
        target_family="wasm" if role == "wasm-ld" else "native",
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
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )
    monkeypatch.setattr(
        llvm_wasi_tools, "_managed_llvm_bin_directories", lambda _root, **_kwargs: ()
    )

    assert (
        llvm_wasi_tools.llvm_linker_candidates(
            requested,
            target_family="wasm" if requested == "wasm-ld" else "native",
            explicit_commands=((str(wrong_path),),),
        )
        == ()
    )


def test_tool_family_resolves_managed_target_root_before_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    managed = _write_tool_family(tmp_path / "target" / "toolchains" / "llvm-22" / "bin")
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **_kwargs: "22.1.8"
    )
    monkeypatch.setattr(
        llvm_wasi_tools,
        "find_executable",
        lambda name, **_kwargs: f"/path/{name}",
    )

    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="wasm", target_root=tmp_path / "target"
    )

    assert family.missing_roles() == ()
    assert family.cc is not None
    assert family.cc.path == managed["cc"].resolve()
    assert family.nm is not None
    assert family.nm.path == managed["nm"].resolve()


@pytest.mark.parametrize("first", ["native", "wasm"])
def test_managed_sdk_discovery_is_target_scoped_in_both_cache_orders(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first: str
) -> None:
    target = tmp_path / "target"
    native = _write_tool_family(target / "toolchains" / "llvm-22" / "bin")
    sdk = _write_tool_family(target / "toolchains" / "wasi-sdk" / "bin")
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    environment = {"PATH": "", "NoDefaultCurrentDirectoryInExePath": "1"}
    for family in (first, "wasm" if first == "native" else "native", first):
        candidates = llvm_wasi_tools.llvm_tool_candidates(
            "cc", target_family=family, target_root=target, environment=environment
        )
        assert candidates[0] == (native if family == "native" else sdk)["cc"]
        assert (sdk["cc"] in candidates) is (family == "wasm")


@pytest.mark.parametrize("origin", ["path", "sibling"])
@pytest.mark.parametrize("native_present", [False, True])
def test_native_discovery_rejects_automatic_sdk_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, origin: str, native_present: bool
) -> None:
    sdk = _write_tool_family(tmp_path / "wasi-sdk-33" / "bin")
    native = _write_tool_family(tmp_path / "native" / "bin")
    for path in (*sdk.values(), *native.values()):
        path.chmod(0o755)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    directories = [sdk["cc"].parent]
    if native_present:
        directories.append(native["cc"].parent)
    environment = {
        "PATH": os.pathsep.join(map(str, directories)) if origin == "path" else "",
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    before = dict(environment)
    siblings = tuple(directories) if origin == "sibling" else ()
    assert llvm_wasi_tools.llvm_tool_candidates(
        "cc", sibling_directories=siblings, environment=environment
    ) == ((native["cc"],) if native_present else ())
    assert (
        llvm_wasi_tools.llvm_tool_candidates(
            "cc",
            target_family="wasm",
            sibling_directories=siblings,
            environment=environment,
        )[0]
        == sdk["cc"]
    )
    assert environment == before


@pytest.mark.parametrize(
    "selector",
    ["WASI_SDK_PATH", "WASI_SDK_PREFIX", "MOLT_WASI_SYSROOT", "WASI_SYSROOT"],
)
def test_relocated_sdk_selection_needs_no_path_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, selector: str
) -> None:
    root = tmp_path / "external-prefix" / "sdk"
    sdk = _write_tool_family(root / "bin")
    sysroot = root / "share" / "wasi-sysroot"
    sysroot.mkdir(parents=True)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    environment = {
        "PATH": str(root / "bin"),
        selector: str(sysroot if "SYSROOT" in selector else root),
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == ()
    environment["PATH"] = ""
    assert llvm_wasi_tools.llvm_tool_candidates(
        "cc", target_family="wasm", environment=environment
    ) == (sdk["cc"],)
    assert environment["PATH"] == ""


def test_sdk_file_alias_does_not_hide_later_native_path_compiler(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sdk = _write_tool_family(tmp_path / "wasi-sdk" / "bin")
    native = _write_tool_family(tmp_path / "native" / "bin")
    alias_directory = tmp_path / "aliases"
    alias_directory.mkdir()
    alias = alias_directory / sdk["cc"].name
    try:
        alias.symlink_to(sdk["cc"])
    except OSError:
        pytest.skip("file symlink privilege unavailable")
    for path in (*sdk.values(), *native.values()):
        path.chmod(0o755)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    environment = {
        "PATH": os.pathsep.join(map(str, (alias_directory, native["cc"].parent))),
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == (
        native["cc"],
    )
    assert llvm_wasi_tools.llvm_tool_candidates(
        "cc", target_family="wasm", environment=environment
    ) == (alias,)
    assert (
        llvm_wasi_tools.llvm_tool_candidates(
            "cc", explicit_commands=((str(alias),),), environment=environment
        )[0]
        == alias
    )


@pytest.mark.skipif(os.name != "nt", reason="Windows PATH and implicit cwd semantics")
def test_native_sdk_path_filter_reuses_windows_executable_search_policy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sdk = _write_tool_family(tmp_path / "wasi-sdk" / "bin")
    native = _write_tool_family(tmp_path / "native" / "bin")
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.chdir(sdk["cc"].parent)
    path = f'"{sdk["cc"].parent}";"{native["cc"].parent}"'
    environment = {"Path": path, "PATH": path, "PathExt": ".EXE"}
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == (
        native["cc"],
    )
    assert environment == {"Path": path, "PATH": path, "PathExt": ".EXE"}


def test_explicit_sdk_compiler_does_not_authorize_automatic_native_siblings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sdk = _write_tool_family(tmp_path / "wasi-sdk" / "bin")
    native = _write_tool_family(tmp_path / "native" / "bin")
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **_kw: "fixture"
    )
    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="native",
        explicit_commands={"cc": (str(sdk["cc"]), "-DEXPLICIT=1")},
        sibling_directories=(sdk["cc"].parent, native["cc"].parent),
        environment={"PATH": "", "NoDefaultCurrentDirectoryInExePath": "1"},
    )
    assert family.cc.command == (str(sdk["cc"]), "-DEXPLICIT=1")
    assert family.cxx.path == native["cxx"]
    assert family.nm.path == native["nm"]


@pytest.mark.parametrize("configured_cc", [None, "", "explicit-sdk"])
def test_native_source_extension_default_and_explicit_compiler_share_target_policy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, configured_cc: str | None
) -> None:
    target = tmp_path / "target"
    sdk = _write_tool_family(target / "toolchains" / "wasi-sdk" / "bin")
    native = _write_tool_family(target / "toolchains" / "llvm-22" / "bin")
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **_kw: "fixture"
    )
    environment = {
        "MOLT_TARGET_ROOT": str(target),
        "PATH": str(sdk["cc"].parent),
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    if configured_cc is not None:
        environment["CC"] = '"' + str(sdk["cc"]) + '"' if configured_cc else ""
    plan = source_extension_target.resolve_source_extension_target_plan(
        "native", host_platform="linux", host_arch="x86_64"
    )
    resolved = source_extension_toolchain._resolve_source_extension_native_toolchain(
        plan, environment=environment
    )
    expected = sdk if configured_cc else native
    for role, executable in (
        ("c", expected["cc"]),
        ("cpp", native["cxx"]),
        ("nm", native["nm"]),
    ):
        assert Path(resolved.commands[role][0]) == executable
        assert resolved.commands[role][1:] == ()


@pytest.mark.parametrize("requested", ["wasm", "wasm-freestanding"])
@pytest.mark.parametrize(
    "selection", ["automatic", "explicit", "directory-alias", "file-alias"]
)
def test_sdk_source_compilers_disable_hidden_config_before_probe_and_materialization(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, requested: str, selection: str
) -> None:
    root = tmp_path / "external-prefix" / "sdk"
    sdk = _write_tool_family(root / "bin")
    sysroot = root / "share" / "wasi-sysroot"
    sysroot.mkdir(parents=True)
    (root / "bin" / "clang.cfg").write_text(
        "--sysroot=<CFGDIR>/../share/wasi-sysroot\n", encoding="utf-8"
    )
    compiler = sdk["cc"]
    if selection == "directory-alias":
        alias = tmp_path / "selected-bin"
        try:
            alias.symlink_to(root / "bin", target_is_directory=True)
        except OSError:
            pytest.skip("directory symlink privilege unavailable")
        compiler = alias / compiler.name
    elif selection == "file-alias":
        alias = tmp_path / compiler.name
        try:
            alias.symlink_to(compiler)
        except OSError:
            pytest.skip("file symlink privilege unavailable")
        compiler = alias
    environment = {
        "WASI_SDK_PATH": str(root),
        "PATH": "",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    if selection != "automatic":
        environment["MOLT_WASM_CC"] = '"' + str(compiler) + '"'
        if requested == "wasm":
            environment["MOLT_WASM_CC"] += ' --sysroot "' + str(sysroot) + '"'
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **_kw: "fixture"
    )
    monkeypatch.setattr(
        source_extension_toolchain, "_resolve_wasi_sysroot", lambda **_kw: sysroot
    )
    monkeypatch.setattr(
        source_extension_toolchain, "normalize_wasi_sysroot", lambda path: Path(path)
    )
    probes = []

    def run(command, **kwargs):
        assert kwargs["env"] == environment
        probes.append(tuple(command))
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(source_extension_toolchain.subprocess, "run", run)
    plan = source_extension_target.resolve_source_extension_target_plan(
        requested, host_platform="linux", host_arch="x86_64"
    )
    resolved = source_extension_toolchain._resolve_source_extension_wasm_toolchain(
        plan, environment=environment
    )
    assert resolved.ok, resolved.detail
    commands = source_extension_toolchain._source_extension_c_commands(
        toolchain=resolved, target_plan=plan, environment=environment
    )
    assert len(probes) == 1
    assert Path(probes[0][0]) == compiler
    for command in (probes[0], commands["c"], commands["cpp"]):
        assert command.count("--no-default-config") == 1
        assert command.count(plan.target_triple) == 1
        assert ("--sysroot" in command) is (requested == "wasm")
        if requested == "wasm":
            assert command[command.index("--sysroot") + 1] == str(sysroot)


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
    monkeypatch.setattr(
        llvm_wasi_tools, "find_executable", lambda _name, **_kwargs: None
    )

    assert llvm_wasi_tools.llvm_tool_candidates("cc")[0] == managed["cc"].resolve()


def test_sdk_provenance_reuses_resolution_and_layout_across_roles_and_warm_hits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sdk = _write_tool_family(tmp_path / "wasi-sdk" / "bin")
    native = _write_tool_family(tmp_path / "native" / "bin")
    for path in (*sdk.values(), *native.values()):
        path.chmod(0o755)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    environment = {
        "WASI_SDK_PATH": str(sdk["cc"].parent.parent),
        "PATH": os.pathsep.join(map(str, (sdk["cc"].parent, native["cc"].parent))),
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    resolutions = []
    layout_probes = []
    resolve = Path.resolve
    is_dir = Path.is_dir

    def observed_resolve(path, *args, **kwargs):
        resolutions.append(path)
        return resolve(path, *args, **kwargs)

    def observed_is_dir(path):
        layout_probes.append(path)
        return is_dir(path)

    monkeypatch.setattr(Path, "resolve", observed_resolve)
    monkeypatch.setattr(Path, "is_dir", observed_is_dir)
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == (
        native["cc"],
    )
    assert resolutions and layout_probes
    resolutions.clear()
    layout_probes.clear()
    assert llvm_wasi_tools.llvm_tool_candidates("cxx", environment=environment) == (
        native["cxx"],
    )
    assert resolutions == [native["cxx"]]
    assert layout_probes == []
    resolutions.clear()
    for role in ("cc", "cxx", "cc"):
        assert llvm_wasi_tools.llvm_tool_candidates(role, environment=environment) == (
            native[role],
        )
    assert resolutions == []
    assert layout_probes == []


@pytest.mark.parametrize("alias_kind", ["directory", "file"])
@pytest.mark.parametrize("mutation", ["sdk-layout", "retarget"])
def test_sdk_provenance_cache_tracks_alias_and_target_layout_mutations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, alias_kind: str, mutation: str
) -> None:
    original = _write_tool_family(tmp_path / "relocated" / "bin")
    sdk = _write_tool_family(tmp_path / "wasi-sdk" / "bin")
    fallback = _write_tool_family(tmp_path / "native" / "bin")
    for path in (*original.values(), *sdk.values(), *fallback.values()):
        path.chmod(0o755)
    if alias_kind == "directory":
        alias = tmp_path / "alias-bin"
        original_target = original["cc"].parent
        sdk_target = sdk["cc"].parent
        search_directory = alias
        compiler = alias / original["cc"].name
    else:
        search_directory = tmp_path / "alias-bin"
        search_directory.mkdir()
        alias = search_directory / original["cc"].name
        original_target = original["cc"]
        sdk_target = sdk["cc"]
        compiler = alias
    try:
        alias.symlink_to(original_target, target_is_directory=alias_kind == "directory")
    except OSError:
        pytest.skip("symlink privilege unavailable")
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    environment = {
        "PATH": os.pathsep.join(map(str, (search_directory, fallback["cc"].parent))),
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }

    def candidates(family):
        return llvm_wasi_tools.llvm_tool_candidates(
            "cc", target_family=family, environment=environment
        )

    assert candidates("native") == (compiler,)
    assert candidates("wasm") == (compiler,)
    if mutation == "sdk-layout":
        sysroot = original["cc"].parent.parent / "share" / "wasi-sysroot"
        sysroot.mkdir(parents=True)
    else:
        alias.unlink()
        alias.symlink_to(sdk_target, target_is_directory=alias_kind == "directory")
    assert candidates("native") == (fallback["cc"],)
    assert candidates("wasm") == (compiler,)
    if mutation == "sdk-layout":
        sysroot.rmdir()
    else:
        alias.unlink()
        alias.symlink_to(original_target, target_is_directory=alias_kind == "directory")
    assert candidates("native") == (compiler,)
    assert candidates("wasm") == (compiler,)


def test_candidate_resolution_memoizes_filesystem_candidate_probes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    clang = _tool_path(tmp_path / "path-bin", "cc")
    clang.parent.mkdir()
    clang.write_bytes(b"clang")
    calls: list[str] = []
    managed_calls = 0

    def managed(_target_root: Path | None, **_kwargs) -> tuple[Path, ...]:
        nonlocal managed_calls
        managed_calls += 1
        return ()

    def which(name: str, **_kwargs) -> str | None:
        calls.append(name)
        return str(clang) if name == "clang" else None

    monkeypatch.setattr(llvm_wasi_tools, "_managed_llvm_bin_directories", managed)
    monkeypatch.setattr(llvm_wasi_tools, "find_executable", which)

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
        lambda _target_root, **_kwargs: (managed_dir,),
    )

    def which(name: str, **_kwargs) -> str | None:
        assert name == "clang"
        return str(path_a if os.environ["PATH"] == "A" else path_b)

    monkeypatch.setattr(llvm_wasi_tools, "find_executable", which)
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
        lambda _target_root, **_kwargs: (managed_dir,),
    )
    monkeypatch.setattr(
        llvm_wasi_tools,
        "find_executable",
        lambda name, **_kwargs: str(fallback) if name == "clang" else None,
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
        lambda _target_root, **_kwargs: (),
    )
    monkeypatch.setattr(
        llvm_wasi_tools,
        "find_executable",
        lambda name, **_kwargs: str(fallback) if name == "clang" else None,
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


def test_captured_search_environment_controls_execution_and_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured_paths = _write_tool_family(tmp_path / "captured")
    ambient_paths = _write_tool_family(tmp_path / "ambient")
    for path in (*captured_paths.values(), *ambient_paths.values()):
        path.chmod(0o755)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.setenv("PATH", str(ambient_paths["cc"].parent))
    environment = {
        "PATH": str(captured_paths["cc"].parent),
        "PATHEXT": ".EXE",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    command = resolve_explicit_tool_command(
        "clang --version", label="captured compiler", environment=environment
    )
    assert Path(command[0]) == captured_paths["cc"].absolute()
    assert command[1:] == ("--version",)
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == (
        captured_paths["cc"].absolute(),
    )
    before = llvm_wasi_tools.llvm_tool_candidate_cache_info()
    monkeypatch.setenv("PATH", "ambient changed after capture")
    monkeypatch.setenv("PATHEXT", ".AMBIENT")
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment=environment) == (
        captured_paths["cc"].absolute(),
    )
    assert (
        llvm_wasi_tools.llvm_tool_candidate_cache_info()["hits"] == before["hits"] + 1
    )
    assert llvm_wasi_tools.llvm_linker_candidates(
        "wasm-ld", target_family="wasm", environment=environment
    ) == (captured_paths["wasm_ld"].absolute(),)


def test_empty_captured_environment_never_uses_ambient_search(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient = _write_tool_family(tmp_path / "ambient")
    for path in ambient.values():
        path.chmod(0o755)
    monkeypatch.setenv("PATH", str(ambient["cc"].parent))
    monkeypatch.delenv("MOLT_TARGET_ROOT", raising=False)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    assert llvm_wasi_tools.llvm_tool_candidates("cc", environment={}) == ()
    with pytest.raises(ValueError, match="not found on PATH"):
        resolve_explicit_tool_command("clang", label="compiler", environment={})


def test_captured_managed_target_root_owns_candidate_precedence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured_root = tmp_path / "captured-target"
    ambient_root = tmp_path / "ambient-target"
    captured = _write_tool_family(captured_root / "toolchains" / "llvm-22" / "bin")
    _write_tool_family(ambient_root / "toolchains" / "llvm-99" / "bin")
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(ambient_root))
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    assert llvm_wasi_tools.llvm_tool_candidates(
        "cc",
        environment={
            "MOLT_TARGET_ROOT": str(captured_root),
            "PATH": "",
            "PATHEXT": ".EXE",
        },
    ) == (captured["cc"].absolute(),)


@pytest.mark.parametrize("selector", ["managed", "target_root", "sibling", "explicit"])
def test_llvm_user_selectors_use_selected_home_and_cache_identity(
    tmp_path, monkeypatch, selector
):
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    homes = [tmp_path / "first", tmp_path / "second"]
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    for home in homes:
        relative = (
            Path("target/toolchains/llvm-22/bin")
            if selector in {"managed", "target_root"}
            else Path("bin")
        )
        paths = _write_tool_family(home / relative)
        environment = {home_key: str(home), "PATH": "", "PATHEXT": ".EXE"}
        kwargs = {}
        if selector == "managed":
            environment["MOLT_TARGET_ROOT"] = "~/target"
        elif selector == "target_root":
            kwargs["target_root"] = Path("~/target")
        elif selector == "sibling":
            kwargs["sibling_directories"] = (Path("~/bin"),)
        else:
            kwargs["explicit_commands"] = ((f"~/bin/{paths['cc'].name}", "-c"),)
        assert llvm_wasi_tools.llvm_tool_candidates(
            "cc", environment=environment, **kwargs
        ) == (paths["cc"],)


def test_llvm_family_materializes_selected_user_command(tmp_path, monkeypatch):
    home = tmp_path / "selected"
    paths = _write_tool_family(home / "bin")
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    environment = {home_key: str(home), "PATH": "", "PATHEXT": ".EXE"}
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, *, environment: "fixture"
    )
    family = llvm_wasi_tools.resolve_llvm_wasi_tool_family(
        target_family="native",
        explicit_commands={"cc": (f"~/bin/{paths['cc'].name}", "-DSELECTED=1")},
        environment=environment,
    )
    assert family.cc.command == (str(paths["cc"]), "-DSELECTED=1")
    assert family.cxx.path == paths["cxx"]


@pytest.mark.skipif(
    os.name != "nt", reason="Windows executable suffix/current-directory contract"
)
def test_windows_captured_pathext_and_cwd_policy_override_ambient(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "selected"
    directory.mkdir()
    selected = directory / "clang.ALT"
    selected.write_bytes(b"captured suffix")
    (directory / "clang.exe").write_bytes(b"ambient suffix")
    current = tmp_path / "current"
    current.mkdir()
    (current / "clang.ALT").write_bytes(b"implicit current directory")
    monkeypatch.chdir(current)
    monkeypatch.setenv("PATHEXT", ".EXE")
    monkeypatch.delenv("NoDefaultCurrentDirectoryInExePath", raising=False)
    monkeypatch.setattr(llvm_wasi_tools, "_source_checkout_roots", lambda: ())
    captured = {
        "Path": str(directory),
        "PathExt": ".ALT",
        "NoDefaultCurrentDirectoryInExePath": "1",
    }
    command = resolve_explicit_tool_command(
        "clang", label="compiler", environment=captured
    )
    assert len(command) == 1
    assert Path(command[0]) == selected.absolute()
    assert llvm_wasi_tools.llvm_named_tool_candidates(
        "clang", environment=captured
    ) == (selected.absolute(),)
    captured.pop("NoDefaultCurrentDirectoryInExePath")
    assert llvm_wasi_tools.llvm_named_tool_candidates(
        "clang", environment=captured
    ) == ((current / "clang.ALT").absolute(),)


def test_captured_rust_tool_lookup_threads_actual_executable_and_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import subprocess

    sysroot = tmp_path / "rust-sysroot"
    reader = sysroot / "lib" / "rustlib" / "host" / "bin" / "llvm-nm"
    reader.parent.mkdir(parents=True)
    reader.write_bytes(b"reader")
    selected_rustc = tmp_path / "selected-rustc"
    environment = {
        "RUSTC": str(selected_rustc),
        "PATH": "captured",
        "RUSTUP_HOME": "captured rustup",
    }
    seen: list[tuple[list[str], object]] = []
    monkeypatch.setattr(
        llvm_wasi_tools, "resolve_executable", lambda command, **kwargs: Path(command)
    )

    def run(command, **kwargs):
        seen.append((command, kwargs["env"]))
        output = "rustc fixture\nhost: host\n" if "-vV" in command else str(sysroot)
        return subprocess.CompletedProcess(command, 0, output, "")

    monkeypatch.setattr(llvm_wasi_tools, "_run_completed_command", run)
    assert llvm_wasi_tools._rust_llvm_bin_directories(environment=environment) == (
        reader.parent,
    )
    assert seen == [
        ([str(selected_rustc), "--print", "sysroot"], environment),
        ([str(selected_rustc), "-vV"], environment),
    ]


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
        target_family: llvm_wasi_tools.LlvmTargetFamily,
        explicit_commands: dict[llvm_wasi_tools.LlvmToolRole, tuple[str, ...]],
        environment: object,
    ) -> llvm_wasi_tools.LlvmWasiToolFamily:
        del environment
        assert target_family == "wasm"
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
        lambda _raw, *, label, environment: compiler,
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
        lambda _command, *, target_plan, environment: None,
    )
    target_plan = source_extension_target.resolve_source_extension_target_plan(
        "wasm",
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

    monkeypatch.setattr(native_symbol_inspection, "llvm_tool_candidates", candidates)

    assert native_symbol_inspection._nm_candidate_binaries() == [
        str(Path("/llvm/bin/llvm-nm")),
        str(Path("/usr/bin/nm")),
    ]
    assert seen == [("nm", True)]


def test_wasm_symbol_reader_consumes_only_verified_llvm_nm(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()
    monkeypatch.delenv("MOLT_LLVM_NM", raising=False)
    calls: list[list[str]] = []
    verified_nm = tmp_path / "llvm" / "bin" / "llvm-nm"
    verified_nm.parent.mkdir(parents=True)
    verified_nm.write_bytes(b"tool")
    executable_identity = native_symbol_inspection.stable_regular_file_identity(
        verified_nm, label="test llvm-nm"
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "verify_wasm_llvm_nm",
        lambda _root, *, environ: llvm_toolchain.WasmLlvmNmVerification(
            path=verified_nm,
            fact=llvm_toolchain.LlvmToolVersionFact(
                "llvm-nm", "bin/llvm-nm", "22.1.8", 7, "a" * 64
            ),
            executable_identity=executable_identity,
        ),
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "_run_completed_command",
        lambda argv, **_kwargs: (
            calls.append(argv)
            or subprocess.CompletedProcess(argv, 0, "00000000 T provider\n", "")
        ),
    )

    facts = native_symbol_inspection._read_native_global_symbol_facts(
        Path("libc.a"), timeout=1, target_triple="wasm32-wasip1"
    )
    assert facts.defined == {"provider"}
    assert [call[0] for call in calls] == [str(verified_nm)]


def test_wasm_symbol_reader_warm_verification_recaptures_retargeted_alias(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()
    alias = tmp_path / "bin" / "llvm-nm"
    first = tmp_path / "tool-generations" / "llvm-nm-first"
    second = tmp_path / "tool-generations" / "llvm-nm-second"
    alias.parent.mkdir(parents=True)
    first.parent.mkdir(parents=True)
    first.write_bytes(b"generation-a")
    second.write_bytes(b"generation-b")
    try:
        alias.symlink_to(first)
    except OSError:
        pytest.skip("file symlinks are unavailable")
    calls: list[Path] = []

    def verify(_root, *, environ):
        del environ
        identity = native_symbol_inspection.stable_regular_file_identity(
            alias.resolve(strict=True),
            label="test llvm-nm generation",
        )
        calls.append(identity.path)
        return llvm_toolchain.WasmLlvmNmVerification(
            path=alias,
            fact=llvm_toolchain.LlvmToolVersionFact(
                "llvm-nm",
                "external:llvm-nm",
                "22.1.8",
                identity.size,
                identity.sha256,
            ),
            executable_identity=identity,
        )

    monkeypatch.setattr(native_symbol_inspection, "verify_wasm_llvm_nm", verify)
    environment = {"MOLT_LLVM_NM": str(alias), "PATH": ""}
    initial = native_symbol_inspection._verified_wasm_llvm_nm(environment)

    alias.unlink()
    alias.symlink_to(second)
    refreshed = native_symbol_inspection._verified_wasm_llvm_nm(environment)

    assert initial.executable_identity.path == first.absolute()
    assert refreshed.executable_identity.path == second.absolute()
    assert calls == [first.absolute(), second.absolute()]
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()


def test_wasm_archive_cache_identity_includes_verified_reader_attestation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()
    monkeypatch.delenv("MOLT_LLVM_NM", raising=False)
    artifact = tmp_path / "libc.a"
    artifact.write_bytes(b"archive")
    verified_nm = tmp_path / "llvm" / "bin" / "llvm-nm"
    verified_nm.parent.mkdir(parents=True)
    verified_nm.write_bytes(b"tool")
    executable_identity = native_symbol_inspection.stable_regular_file_identity(
        verified_nm, label="test llvm-nm"
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    reader_digest = "a" * 64

    def verify(_root, *, environ):
        return llvm_toolchain.WasmLlvmNmVerification(
            path=verified_nm,
            fact=llvm_toolchain.LlvmToolVersionFact(
                "llvm-nm", "bin/llvm-nm", "22.1.8", 7, reader_digest
            ),
            executable_identity=executable_identity,
        )

    monkeypatch.setattr(native_symbol_inspection, "verify_wasm_llvm_nm", verify)
    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        lambda *_args, **_kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
            frozenset({"provider"}), frozenset(), frozenset({"provider"})
        ),
    )

    native_symbol_inspection._native_archive_global_symbol_facts(
        artifact, target_triple="wasm32-wasip1"
    )
    first = next(iter(native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE))
    assert first[8][-4:] == (
        "22.1.8",
        "a" * 64,
        "",
        native_symbol_inspection.NativeSymbolRequirement().cache_identity(),
    )

    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    reader_digest = "b" * 64
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()
    native_symbol_inspection._native_archive_global_symbol_facts(
        artifact, target_triple="wasm32-wasip1"
    )
    second = next(iter(native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE))
    assert first != second


def test_wasm_symbol_reader_rejects_tool_replacement_during_inspection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    native_symbol_inspection._cached_wasm_llvm_nm_verification.cache_clear()
    monkeypatch.delenv("MOLT_LLVM_NM", raising=False)
    verified_nm = tmp_path / "llvm-nm"
    verified_nm.write_bytes(b"generation-a")
    executable_identity = native_symbol_inspection.stable_regular_file_identity(
        verified_nm, label="test llvm-nm"
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "verify_wasm_llvm_nm",
        lambda _root, *, environ: llvm_toolchain.WasmLlvmNmVerification(
            path=verified_nm,
            fact=llvm_toolchain.LlvmToolVersionFact(
                "llvm-nm", "external:llvm-nm", "22.1.8", 12, "a" * 64
            ),
            executable_identity=executable_identity,
        ),
    )

    def replace_reader(argv, **_kwargs):
        verified_nm.write_bytes(b"generation-b")
        return subprocess.CompletedProcess(argv, 0, "00000000 T provider\n", "")

    monkeypatch.setattr(
        native_symbol_inspection, "_run_completed_command", replace_reader
    )

    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="verified symbol reader changed during symbol inspection",
    ):
        native_symbol_inspection._read_native_global_symbol_facts(
            tmp_path / "libc.a",
            timeout=1,
            target_triple="wasm32-wasip1",
        )


def test_native_symbol_reader_rejects_tool_replacement_during_inspection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    native_symbol_inspection._cached_symbol_reader_entrypoint_identity.cache_clear()
    native_nm = tmp_path / "nm"
    native_nm.write_bytes(b"generation-a")
    monkeypatch.setattr(
        native_symbol_inspection, "_nm_candidate_binaries", lambda: [str(native_nm)]
    )

    def replace_reader(argv, **_kwargs):
        native_nm.write_bytes(b"generation-b")
        return subprocess.CompletedProcess(argv, 0, "00000000 T provider\n", "")

    monkeypatch.setattr(
        native_symbol_inspection, "_run_completed_command", replace_reader
    )

    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="verified symbol reader changed during symbol inspection",
    ):
        native_symbol_inspection._read_native_global_symbol_facts(
            tmp_path / "native.a",
            timeout=1,
        )

"""Source-extension queue identities project the canonical selected family."""

from __future__ import annotations

from dataclasses import replace
import hashlib
import os
from pathlib import Path
import subprocess

import pytest

from molt.cli import llvm_wasi_tools, source_extension_toolchain, wasm_link_inputs
from molt.cli.source_extension_compiler_inputs import compiler_sysroot_arg_value
from molt.cli.source_extension_set_validation_target import (
    _source_extension_tool_role_contract,
)
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt.exact_json import canonical_json_sha256
from molt.source_extension_link_inputs import SourceExtensionLinkInputs
from tools import proof_plan
from tools.proof_queue_pkg import process_image_capture, toolchain_capture
from tools.proof_queue_pkg import target_derived_toolchains as provider


def _policy(**updates):
    return proof_plan.ToolchainPolicy(
        "source-extension",
        {
            "identity_provider": "source-extension",
            "version_pattern": "molt-source-extension-toolchain-v2",
            **updates,
        },
    )


def _native_tool(root, role, name):
    path = root / (name + (".exe" if os.name == "nt" else ""))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"MZ\0\0" + name.encode())
    path.chmod(0o755)
    return llvm_wasi_tools.ResolvedLlvmTool(
        role=role,
        command=(str(path),),
        path=path,
        version="fixture-native-image",
        sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )


def _resolved(root, requested="native"):
    target = resolve_source_extension_target_plan(requested)
    names = {
        "cc": "clang",
        "cxx": "clang++",
        "ar": "llvm-ar",
        "nm": "llvm-nm",
        "wasm_ld": "wasm-ld",
        "ranlib": "llvm-ranlib",
        "strip": "llvm-strip",
    }
    family = llvm_wasi_tools.LlvmWasiToolFamily(
        **{role: _native_tool(root / "bin", role, name) for role, name in names.items()}
    )
    roles, _ = _source_extension_tool_role_contract(target.target_triple)
    commands = {
        role: getattr(family, tool_role).command
        for role, tool_role in roles.items()
        if role != "ld" or target.is_wasm
    }
    sysroot = None
    archive = None
    if target.target_triple == "wasm32-wasip1":
        sysroot = root / "wasi-sysroot"
        (sysroot / "include").mkdir(parents=True)
        (sysroot / "include" / "errno.h").write_text("#define EINVAL 22\n")
        archive = root / "libcompiler_builtins-fixture.rlib"
        archive.write_bytes(b"!<arch>\nfixture builtins")
    for role in ("c", "cpp"):
        if target.compiler_target_triple is not None:
            commands[role] += ("-target", target.target_triple)
        if sysroot is not None:
            commands[role] += ("--sysroot", str(sysroot))
    return source_extension_toolchain._ResolvedSourceExtensionToolchain(
        target_plan=target,
        compiler_kind="fixture",
        tools=family,
        commands=commands,
        wasi_sysroot=sysroot,
        link_inputs=SourceExtensionLinkInputs(
            target.target_triple,
            archive,
            hashlib.sha256(archive.read_bytes()).hexdigest() if archive else None,
            archive.stat().st_size if archive else None,
        ),
        detail="fixture family",
    )


def _capture(monkeypatch, resolved, *, environment=None):
    calls = []

    def resolve(target, *, environment=None):
        assert target == resolved.target_plan
        calls.append(environment)
        return resolved

    monkeypatch.setattr(provider, "_resolve_source_extension_toolchain", resolve)
    identity = provider.capture_identity(
        _policy(),
        {
            "typed_command": {
                "family": "source-extension-producer",
                "target": resolved.target_plan.requested,
            }
        },
        environment=environment,
    )
    assert calls == [environment]
    return identity


def _rehash(identity):
    identity["tool_family_sha256"] = canonical_json_sha256(
        {"tools": identity["tools"], "commands": identity["commands"]}
    )
    identity["identity_sha256"] = canonical_json_sha256(
        {key: value for key, value in identity.items() if key != "identity_sha256"}
    )


@pytest.mark.parametrize("target", ["native", "wasm", "wasm-freestanding"])
def test_capture_records_family_and_frozen_inputs(tmp_path, monkeypatch, target):
    resolved = _resolved(tmp_path, target)
    selected = {"PATH": str(tmp_path / "selected"), "MOLT_CROSS_CC": "selected"}
    before = dict(os.environ)
    identity = _capture(monkeypatch, resolved, environment=selected)
    assert dict(os.environ) == before
    assert identity["schema"] == provider.SOURCE_EXTENSION_SCHEMA
    assert identity["version"] == provider.SOURCE_EXTENSION_VERSION
    assert "path" not in identity and "executable_sha256" not in identity
    images = process_image_capture.toolchain_images("source-extension", identity)
    assert images == identity["process_images"]
    files = {row.path: row for row in toolchain_capture.frozen_files(identity)}
    assert all(
        str(tool.path) in files
        for tool in (
            resolved.tools.cc,
            resolved.tools.cxx,
            resolved.tools.wasm_ld,
            resolved.tools.ar,
            resolved.tools.nm,
            resolved.tools.ranlib,
            resolved.tools.strip,
        )
    )
    if resolved.wasi_sysroot is not None:
        header = resolved.wasi_sysroot / "include" / "errno.h"
        assert files[str(header)].size == header.stat().st_size
        archive = resolved.link_inputs.compiler_builtins
        assert files[str(archive)].sha256 == resolved.link_inputs.sha256


def test_wasi_metadata_consumes_selected_archive_without_discovery(
    tmp_path, monkeypatch
):
    resolved = _resolved(tmp_path / "toolchain", "wasm")
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda *a, **kw: pytest.fail("metadata rediscovered captured archive"),
    )
    arguments = dict(
        molt_root=Path(__file__).resolve().parents[2],
        out_dir=tmp_path / "metadata",
        target_plan=resolved.target_plan,
        python_version="3.12",
        abi_tier="cpython-abi",
        toolchain=resolved,
    )
    metadata, errors = (
        source_extension_toolchain._materialize_source_extension_target_metadata_with_toolchain(
            **arguments
        )
    )
    assert not errors and metadata is not None
    recorded = metadata.payload["toolchain"]["link_probe_archives"]["compiler_builtins"]
    assert recorded == {
        "path": str(resolved.link_inputs.compiler_builtins),
        "sha256": resolved.link_inputs.sha256,
    }
    assert (
        str(resolved.link_inputs.compiler_builtins).replace("\\", "/")
        in metadata.meson_cross.read_text()
    )
    resolved.link_inputs.compiler_builtins.write_bytes(b"changed archive")
    with pytest.raises(ValueError, match="content changed"):
        source_extension_toolchain._materialize_source_extension_target_metadata_with_toolchain(
            **arguments
        )


@pytest.mark.parametrize("target", ["native", "wasm"])
@pytest.mark.parametrize(
    "selector",
    ["-B/foreign", "-Xclang -load plugin", "-Wl,@inputs.rsp", "-resource-dir=/foreign"],
)
def test_external_selectors_fail_before_compiler_family_or_probe(
    tmp_path, monkeypatch, target, selector
):
    resolved = _resolved(tmp_path, target)
    key = "MOLT_WASM_CC" if target == "wasm" else "CC"
    environment = {key: f'"{resolved.tools.cc.path}" {selector}'}
    monkeypatch.setattr(
        source_extension_toolchain,
        "resolve_llvm_wasi_tool_family",
        lambda **kw: pytest.fail("unadmitted command reached tool probe"),
    )
    monkeypatch.setattr(
        source_extension_toolchain,
        "_probe_wasm_source_extension_compiler",
        lambda *a, **kw: pytest.fail("unadmitted compiler reached execution"),
    )
    with pytest.raises(ValueError, match="custody"):
        source_extension_toolchain._resolve_source_extension_toolchain(
            resolved.target_plan, environment=environment
        )


def test_validation_never_rediscovers_native_target_or_probes(tmp_path, monkeypatch):
    identity = _capture(monkeypatch, _resolved(tmp_path))

    def forbidden(*_args, **_kwargs):
        pytest.fail("recorded identity must not select from inspector host or probe")

    monkeypatch.setattr(provider, "resolve_source_extension_target_plan", forbidden)
    monkeypatch.setattr(provider, "_resolve_source_extension_toolchain", forbidden)
    monkeypatch.setattr(source_extension_toolchain.subprocess, "run", forbidden)
    provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize(
    "triple",
    [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ],
)
def test_recorded_native_target_is_independent_of_inspector_host(
    tmp_path, monkeypatch, triple
):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    identity["target_triple"] = triple
    identity["link_inputs"]["target_triple"] = triple
    _rehash(identity)
    provider.validate_identity(_policy(), identity)


def test_script_disguised_as_native_tool_is_rejected(tmp_path, monkeypatch):
    resolved = _resolved(tmp_path)
    path = resolved.tools.cc.path
    path.write_bytes(b"#!/bin/sh\nexec unowned-compiler\n")
    cc = replace(
        resolved.tools.cc, sha256=hashlib.sha256(path.read_bytes()).hexdigest()
    )
    resolved = replace(resolved, tools=replace(resolved.tools, cc=cc))
    with pytest.raises(ValueError, match="must be a native executable"):
        _capture(monkeypatch, resolved)


@pytest.mark.parametrize(
    "name, suffix",
    [
        ("sccache", ()),
        ("ccache", ()),
        ("distcc", ()),
        ("zig", ("cc",)),
    ],
)
def test_uninventoried_compiler_launchers_are_rejected(
    tmp_path, monkeypatch, name, suffix
):
    resolved = _resolved(tmp_path)
    cc = _native_tool(tmp_path / "wrapper", "cc", name)
    cc = replace(cc, command=(*cc.command, *suffix))
    resolved = replace(
        resolved,
        tools=replace(resolved.tools, cc=cc),
        commands={**resolved.commands, "c": cc.command},
    )
    with pytest.raises(ValueError, match="helper-process custody"):
        _capture(monkeypatch, resolved)


def test_changed_executable_rejected_even_with_resealed_outer_digest(
    tmp_path, monkeypatch
):
    resolved = _resolved(tmp_path)
    identity = _capture(monkeypatch, resolved)
    resolved.tools.cc.path.write_bytes(b"MZ\0\0changed")
    _rehash(identity)
    with pytest.raises(ValueError, match="executable content changed"):
        provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize("target", ["native", "wasm-freestanding"])
@pytest.mark.parametrize("role", ["c", "cpp"])
def test_resealed_foreign_compiler_target_is_rejected(
    tmp_path, monkeypatch, target, role
):
    identity = _capture(monkeypatch, _resolved(tmp_path, target))
    identity["commands"][role].append("--target=aarch64-unknown-linux-gnu")
    _rehash(identity)
    with pytest.raises(ValueError, match="target conflicts"):
        provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize("flag", ["@unowned.rsp", "/clang:@unowned.rsp"])
def test_indirect_compiler_inputs_fail_closed(tmp_path, monkeypatch, flag):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    identity["commands"]["c"].append(flag)
    _rehash(identity)
    with pytest.raises(ValueError, match="custody"):
        provider.validate_identity(_policy(), identity)


def test_wasi_sysroot_bytes_are_identity_inputs(tmp_path, monkeypatch):
    resolved = _resolved(tmp_path, "wasm")
    identity = _capture(monkeypatch, resolved)
    (resolved.wasi_sysroot / "include" / "errno.h").write_text("#define EINVAL 999\n")
    with pytest.raises(ValueError, match="sysroot content changed"):
        provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize("mutation", ["missing", "conflicting", "implicit"])
def test_wasi_sysroot_selectors_cannot_escape_custody(tmp_path, monkeypatch, mutation):
    identity = _capture(monkeypatch, _resolved(tmp_path, "wasm"))
    command = identity["commands"]["c"]
    if mutation == "missing":
        command.pop()
    elif mutation == "conflicting":
        command.append("--sysroot=" + str(tmp_path / "other"))
    else:
        del command[-2:]
    _rehash(identity)
    with pytest.raises(ValueError, match="sysroot"):
        provider.validate_identity(_policy(), identity)


def test_non_wasi_unowned_sysroot_fails_closed(tmp_path, monkeypatch):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    identity["commands"]["c"].append("--sysroot=" + str(tmp_path))
    _rehash(identity)
    with pytest.raises(ValueError, match="unowned sysroot"):
        provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize("mutation", ["missing", "extra", "digest", "terminate"])
def test_image_projection_requires_exact_family(tmp_path, monkeypatch, mutation):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    images = identity["process_images"]
    if mutation == "missing":
        images.pop()
    elif mutation == "extra":
        images.append({**images[0], "role": "unowned-helper"})
    elif mutation == "digest":
        images[0]["sha256"] = "0" * 64
    else:
        images[0]["root_exit_disposition"] = "terminate"
    _rehash(identity)
    with pytest.raises(ValueError, match="process images differ"):
        process_image_capture.toolchain_images("source-extension", identity)


def test_linker_alias_preserves_invoked_role_and_captures_resolved_bytes(
    tmp_path, monkeypatch
):
    resolved = _resolved(tmp_path, "wasm-freestanding")
    content = _native_tool(tmp_path / "content", "wasm_ld", "lld")
    alias = tmp_path / ("wasm-ld.exe" if os.name == "nt" else "wasm-ld")
    try:
        alias.symlink_to(content.path)
    except OSError as exc:
        pytest.skip(f"file symlinks unavailable: {exc}")
    linker = replace(content, command=(str(alias),), path=alias)
    resolved = replace(
        resolved,
        tools=replace(resolved.tools, wasm_ld=linker),
        commands={**resolved.commands, "ld": linker.command},
    )
    identity = _capture(monkeypatch, resolved)
    assert identity["commands"]["ld"] == [str(alias)]
    images = process_image_capture.toolchain_images("source-extension", identity)
    assert any(
        row["path"] == str(alias) and row["path_kind"] == "selection" for row in images
    )
    assert any(row["path"] == str(content.path) for row in images)
    provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize(
    "updates",
    [
        {"identity_provider": "unknown"},
        {"version_pattern": "molt-source-extension-toolchain-v1"},
        {"process_image_probes": [["--version"]]},
    ],
)
def test_policy_cannot_select_legacy_or_unowned_provider(
    tmp_path, monkeypatch, updates
):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    with pytest.raises(ValueError):
        provider.validate_identity(_policy(**updates), identity)


@pytest.mark.parametrize("requested", ["native", "x86_64-unknown-linux-gnu"])
def test_canonical_native_resolver_uses_only_selected_environment(
    tmp_path, monkeypatch, requested
):
    resolved = _resolved(tmp_path, requested)
    cross = requested != "native"
    cc_key, cxx_key = ("MOLT_CROSS_CC", "MOLT_CROSS_CXX") if cross else ("CC", "CXX")
    selected = {
        "PATH": str(tmp_path / "bin"),
        "PATHEXT": ".EXE",
        cc_key: '"' + str(resolved.tools.cc.path) + '"',
        cxx_key: '"' + str(resolved.tools.cxx.path) + '"',
    }
    monkeypatch.setenv(cc_key, "ambient-must-not-be-selected")
    monkeypatch.setenv(cxx_key, "ambient-must-not-be-selected")
    seen = []

    def family(*, target_family, explicit_commands, sibling_directories, environment):
        assert target_family == "native"
        assert environment == selected
        seen.append(environment)
        return replace(
            resolved.tools,
            **{
                role: replace(getattr(resolved.tools, role), command=command)
                for role, command in explicit_commands.items()
            },
        )

    monkeypatch.setattr(
        source_extension_toolchain, "resolve_llvm_wasi_tool_family", family
    )
    actual = source_extension_toolchain._resolve_source_extension_toolchain(
        resolved.target_plan, environment=selected
    )
    assert actual.commands["c"][0] == str(resolved.tools.cc.path)
    assert actual.commands["cpp"][0] == str(resolved.tools.cxx.path)
    assert seen == [selected]
    assert os.environ[cc_key] == "ambient-must-not-be-selected"


@pytest.mark.parametrize("source", ["MOLT_WASM_CC", "MOLT_CROSS_CC", "discovery"])
def test_canonical_wasm_resolver_passes_environment_to_every_selection(
    tmp_path, monkeypatch, source
):
    resolved = _resolved(tmp_path, "wasm-freestanding")
    selected = {"PATH": str(tmp_path / "bin"), "PATHEXT": ".EXE"}
    if source != "discovery":
        selected[source] = '"' + str(resolved.tools.cc.path) + '"'
    monkeypatch.setenv("MOLT_WASM_CC", "ambient-must-not-be-selected")
    seen = []

    def family(*, target_family, environment, explicit_commands=None):
        assert target_family == "wasm"
        assert environment == selected
        seen.append("family")
        return resolved.tools

    def probe(command, *, target_plan, environment):
        assert environment == selected
        assert target_plan == resolved.target_plan
        seen.append("probe")
        return None

    monkeypatch.setattr(
        source_extension_toolchain, "resolve_llvm_wasi_tool_family", family
    )
    monkeypatch.setattr(
        source_extension_toolchain, "_probe_wasm_source_extension_compiler", probe
    )
    actual = source_extension_toolchain._resolve_source_extension_toolchain(
        resolved.target_plan, environment=selected
    )
    assert actual.commands["c"][0] == str(resolved.tools.cc.path)
    assert seen == ["family", "probe"]
    assert os.environ["MOLT_WASM_CC"] == "ambient-must-not-be-selected"


def test_compiler_probe_subprocess_receives_exact_environment(tmp_path, monkeypatch):
    selected = {"PATH": str(tmp_path), "MOLT_PROBE_TEST": "selected"}
    target = resolve_source_extension_target_plan("wasm-freestanding")
    calls = []

    def run(command, **kwargs):
        assert kwargs["env"] == selected
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(source_extension_toolchain.subprocess, "run", run)
    assert (
        source_extension_toolchain._probe_wasm_source_extension_compiler(
            (str(tmp_path / "clang"),), target_plan=target, environment=selected
        )
        is None
    )
    assert len(calls) == 1


@pytest.mark.parametrize(
    "args",
    [
        ("--sysroot",),
        ("--sysroot=",),
        ("-isysroot", "-target"),
        ("--sysroot", "/one", "-isysroot=/two"),
    ],
)
def test_shared_sysroot_parser_rejects_missing_or_conflicting_selectors(args):
    with pytest.raises(ValueError, match="sysroot"):
        compiler_sysroot_arg_value(args)


def test_shared_sysroot_parser_accepts_repeated_identical_selector():
    assert compiler_sysroot_arg_value(("--sysroot=/one", "-isysroot", "/one")) == "/one"


def test_wasi_sysroot_cache_key_contains_only_selected_windows_roots(
    tmp_path, monkeypatch
):
    selected = {
        "MOLT_WASI_SYSROOT": str(tmp_path / "explicit"),
        "ProgramFiles": str(tmp_path / "programs"),
        "LOCALAPPDATA": str(tmp_path / "local"),
    }
    monkeypatch.setenv("ProgramFiles", str(tmp_path / "ambient-programs"))
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "ambient-local"))
    calls = []

    def cached(*values):
        calls.append(values)
        return None

    monkeypatch.setattr(wasm_link_inputs, "_resolve_wasi_sysroot_cached", cached)
    wasm_link_inputs.resolve_wasi_sysroot(env=selected)
    wasm_link_inputs.resolve_wasi_sysroot(env={})
    assert calls == [
        (
            selected["MOLT_WASI_SYSROOT"],
            None,
            None,
            None,
            None,
            selected["ProgramFiles"],
            selected["LOCALAPPDATA"],
        ),
        (None,) * 7,
    ]


@pytest.mark.parametrize(
    "selector",
    ["MOLT_WASI_SYSROOT", "WASI_SYSROOT", "WASI_SDK_PATH", "MOLT_TARGET_ROOT"],
)
def test_wasi_user_selectors_bind_selected_home_in_cache(
    tmp_path, monkeypatch, selector
):
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    for name in ("first", "second"):
        home = tmp_path / name
        root = home / "sdk"
        if selector == "MOLT_TARGET_ROOT":
            root /= "toolchains/wasi-sysroot"
        (root / "include").mkdir(parents=True)
        (root / "include" / "errno.h").write_text("#define EINVAL 22\n")
        assert (
            wasm_link_inputs.resolve_wasi_sysroot(
                env={home_key: str(home), selector: "~/sdk"}
            )
            == root
        )


@pytest.mark.parametrize(
    "sysroot_flag", ["--sysroot ~/wasi-sysroot", "--sysroot=~/wasi-sysroot"]
)
def test_wasi_compiler_user_sysroot_uses_the_same_selected_path_for_probe_and_identity(
    tmp_path, monkeypatch, sysroot_flag
):
    home = tmp_path / "selected home"
    resolved = _resolved(home, "wasm")
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    selected = {
        home_key: str(home),
        "MOLT_WASM_CC": f'"{resolved.tools.cc.path}" {sysroot_flag}',
    }
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))

    def archive(target, *, environment):
        assert environment == selected
        assert target == resolved.target_plan.target_triple
        return resolved.link_inputs.compiler_builtins

    monkeypatch.setattr(wasm_link_inputs, "wasm_compiler_builtins_archive", archive)
    probes = []

    def family(*, target_family, environment, explicit_commands):
        assert target_family == "wasm"
        assert environment == selected
        return replace(
            resolved.tools,
            cc=replace(resolved.tools.cc, command=explicit_commands["cc"]),
        )

    def probe(command, *, target_plan, environment):
        assert environment == selected
        assert compiler_sysroot_arg_value(command) == str(resolved.wasi_sysroot)
        probes.append(command)
        return None

    monkeypatch.setattr(
        source_extension_toolchain, "resolve_llvm_wasi_tool_family", family
    )
    monkeypatch.setattr(
        source_extension_toolchain, "_probe_wasm_source_extension_compiler", probe
    )
    actual = source_extension_toolchain._resolve_source_extension_toolchain(
        resolved.target_plan, environment=selected
    )
    assert len(probes) == 1
    assert actual.wasi_sysroot == resolved.wasi_sysroot
    assert compiler_sysroot_arg_value(actual.commands["c"]) == str(actual.wasi_sysroot)
    assert compiler_sysroot_arg_value(actual.commands["cpp"]) == str(
        actual.wasi_sysroot
    )


@pytest.mark.skipif(os.name != "nt", reason="Windows system-root discovery")
def test_wasi_sysroot_cached_selection_changes_with_selected_windows_roots(
    tmp_path, monkeypatch
):
    first, second = tmp_path / "first", tmp_path / "second"
    roots = {first / "wasi-sdk", second / "wasi-sdk"}
    monkeypatch.setattr(
        wasm_link_inputs,
        "normalize_wasi_sysroot",
        lambda path: path if path in roots else None,
    )
    wasm_link_inputs._resolve_wasi_sysroot_cached.cache_clear()
    try:
        assert (
            wasm_link_inputs.resolve_wasi_sysroot(env={"ProgramFiles": str(first)})
            == first / "wasi-sdk"
        )
        assert (
            wasm_link_inputs.resolve_wasi_sysroot(env={"ProgramFiles": str(second)})
            == second / "wasi-sdk"
        )
    finally:
        wasm_link_inputs._resolve_wasi_sysroot_cached.cache_clear()


def _owned_manifest(root):
    return {
        "root": str(root),
        "file_count": 1,
        "directories": ["include"],
        "files": [{"relative_path": "include/errno.h", "size": 10, "sha256": "a" * 64}],
        "manifest_sha256": "b" * 64,
    }


def test_owned_directory_projects_relative_members_without_a_second_manifest(tmp_path):
    manifest = _owned_manifest(tmp_path)
    files = toolchain_capture.frozen_files({"sysroot_custody": manifest})
    assert files == [
        toolchain_capture.FrozenFile(
            str(tmp_path / "include" / "errno.h"), "a" * 64, 10
        )
    ]
    assert "path" not in manifest["files"][0]


@pytest.mark.parametrize(
    "relative",
    [
        "",
        ".",
        "..",
        "../escape.h",
        "include/../../escape.h",
        "include/./x.h",
        "/absolute.h",
        "C:/absolute.h",
        "C:relative.h",
        "include\\escape.h",
        "include//errno.h",
        "include/",
        "include/\0errno.h",
        None,
        42,
    ],
)
@pytest.mark.parametrize("location", ["files", "directories"])
def test_owned_directory_rejects_escaping_or_malformed_relative_paths(
    tmp_path, relative, location
):
    manifest = _owned_manifest(tmp_path)
    if location == "files":
        manifest["files"][0]["relative_path"] = relative
    else:
        manifest["directories"] = [relative]
    with pytest.raises(ValueError, match="relative member path"):
        toolchain_capture.frozen_files(manifest)


@pytest.mark.parametrize(
    "field,value",
    [
        ("size", True),
        ("size", -1),
        ("size", "10"),
        ("sha256", "g" * 64),
        ("sha256", None),
    ],
)
def test_owned_directory_rejects_malformed_member_identity(tmp_path, field, value):
    manifest = _owned_manifest(tmp_path)
    manifest["files"][0][field] = value
    with pytest.raises(ValueError, match="member identity is malformed"):
        toolchain_capture.frozen_files(manifest)


def test_owned_directory_rejects_relative_symlink_escape(tmp_path):
    root = tmp_path / "owned"
    outside = tmp_path / "outside"
    root.mkdir()
    outside.mkdir()
    try:
        (root / "include").symlink_to(outside, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlinks unavailable: {exc}")
    with pytest.raises(ValueError, match="escapes its root"):
        toolchain_capture.frozen_files(_owned_manifest(root))

"""Source-extension queue identities project the canonical selected family."""

from __future__ import annotations

from molt.llvm_toolchain import capture_wasi_sdk_selection

from dataclasses import replace
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from types import SimpleNamespace

from tests.operation_probe import same_thread_probe

import pytest

from molt.cli import llvm_wasi_tools, source_extension_toolchain, wasm_link_inputs
from molt.cli.source_extension_compiler_inputs import compiler_sysroot_arg_value
from molt.cli.source_extension_set_validation_target import (
    _source_extension_tool_role_contract,
)
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt import llvm_toolchain
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    provisioned_wasi_sdk_fixture,
    runtime_wasi_c_abi_plan,
)
from molt.exact_json import canonical_json_sha256
from molt.source_extension_link_inputs import SourceExtensionLinkInputs
from tools import proof_plan
from tools.proof_queue_pkg import process_image_capture, toolchain_capture
from tools.proof_queue_pkg import target_derived_toolchains as provider
from tests.process_guard_common import install_module_view


def _policy(**updates):
    return proof_plan.ToolchainPolicy(
        "source-extension",
        {
            "identity_provider": "source-extension",
            "version_pattern": "molt-source-extension-toolchain-v4",
            **updates,
        },
    )


def _native_tool(root, role, name):
    path = root / name
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


def _resolved(root, requested="native", *, host_platform=None, host_arch=None):
    target = resolve_source_extension_target_plan(
        requested, host_platform=host_platform, host_arch=host_arch
    )
    msvc = target.target_triple.endswith("-msvc")
    names = {
        "cc": "clang-cl" if msvc else "clang",
        "cxx": "clang-cl" if msvc else "clang++",
        "ar": "llvm-ar",
        "nm": "llvm-nm",
        "wasm_ld": "wasm-ld",
        "ranlib": "llvm-ranlib",
        "strip": "llvm-strip",
    }
    family = llvm_wasi_tools.LlvmWasiToolFamily(
        **{
            role: _native_tool(
                root / "bin", role, name + (".exe" if os.name == "nt" else "")
            )
            for role, name in names.items()
        }
    )
    roles, _ = _source_extension_tool_role_contract(target.target_triple)
    commands = {
        role: getattr(family, tool_role).command
        for role, tool_role in roles.items()
        if role != "ld" or target.is_wasm
    }
    sysroot = None
    archive = None
    c_abi = None
    if target.target_triple == "wasm32-wasip1":
        c_abi = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(root))
        family = llvm_wasi_tools.LlvmWasiToolFamily(
            **{
                role: llvm_wasi_tools.ResolvedLlvmTool(
                    role=role,
                    command=(str(c_abi.driver.parent / (name + c_abi.driver.suffix)),),
                    path=c_abi.driver.parent / (name + c_abi.driver.suffix),
                    version="fixture-native-image",
                    sha256=hashlib.sha256(
                        (
                            c_abi.driver.parent / (name + c_abi.driver.suffix)
                        ).read_bytes()
                    ).hexdigest(),
                )
                for role, name in names.items()
            }
        )
        commands = {
            role: getattr(family, tool_role).command
            for role, tool_role in roles.items()
        }
        sysroot = c_abi.sysroot
        archive = c_abi.path("compiler_rt")
    for role in ("c", "cpp"):
        if target.compiler_target_triple is not None:
            commands[role] += ("-target", target.target_triple)
        if sysroot is not None:
            commands[role] += ("--sysroot", str(sysroot))
    return source_extension_toolchain._ResolvedSourceExtensionToolchain(
        wasi_c_abi=c_abi,
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
    if resolved.wasi_c_abi is not None:
        environment = {
            **(environment or {}),
            "WASI_SDK_PATH": str(resolved.wasi_c_abi.sdk),
        }

    def resolve(target, *, environment=None):
        assert target == resolved.target_plan
        calls.append(environment)
        return resolved

    monkeypatch.setattr(provider, "_resolve_source_extension_toolchain", resolve)

    def phase_run(command, **kwargs):
        assert "-###" in command
        assert Path(kwargs["cwd"]) != proof_plan.ROOT
        return subprocess.CompletedProcess(
            command, 0, "", " " + json.dumps(command[0]) + ' "-cc1"\n'
        )

    with monkeypatch.context() as phase_patch:
        phase_patch.setattr(
            toolchain_capture, "_COMMANDS", SimpleNamespace(run=phase_run)
        )
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


def _reseal_command_change(identity, role):
    """Forge a coherent phase receipt so command-policy negatives reach policy."""
    record = identity["native_compiler_phases"][role]
    old = record["command"]
    changed = list(identity["commands"][role])
    for probe in record["probes"]:
        assert probe["argv"][: len(old)] == old
        probe["argv"] = changed + probe["argv"][len(old) :]
    record["command"] = changed
    _rehash(identity)


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
    files = {
        process_image_capture._image_path_key(Path(row.path)): row
        for row in toolchain_capture.frozen_files(identity)
    }
    assert all(
        process_image_capture._image_path_key(tool.path) in files
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
        header = resolved.wasi_sysroot / "include" / "wasm32-wasip1" / "errno.h"
        assert process_image_capture._image_path_key(header) not in files
        full = toolchain_capture.capture_wasi_sdk_resources(identity["wasi_sdk"])
        captured = {Path(row.path): row for row in toolchain_capture.frozen_files(full)}
        assert captured[header].size == header.stat().st_size
        archive = resolved.link_inputs.compiler_rt
        assert (
            files[process_image_capture._image_path_key(archive)].sha256
            == resolved.link_inputs.sha256
        )


@pytest.mark.skipif(os.name != "nt", reason="native Windows process coordinate join")
@pytest.mark.parametrize("target", ["native", "wasm", "wasm-freestanding"])
def test_windows_target_family_joins_product_paths_to_image_coordinates(
    tmp_path, monkeypatch, target
):
    physical = tmp_path.resolve(strict=True)
    if len(physical.drive) != 2 or physical.drive[1] != ":":
        pytest.skip("fixture requires a DOS drive")
    root = Path(physical.drive.upper() + str(physical)[len(physical.drive) :])
    resolved = _resolved(root, target)
    identity = _capture(monkeypatch, resolved)
    assert identity["tools"]["cc"]["path"] == str(resolved.tools.cc.path)
    images = provider.family_process_images(identity)
    assert images == identity["process_images"]
    selected = next(row for row in images if row["role"] == "source-extension:cc")
    raw = str(resolved.tools.cc.path)
    assert selected["path"] == raw[0].lower() + raw[1:]
    assert (
        selected["sha256"]
        == hashlib.sha256(resolved.tools.cc.path.read_bytes()).hexdigest()
    )
    assert process_image_capture.revalidate_images(images) == images


@pytest.mark.parametrize(
    "mutation",
    ["none", "launcher-hardlink", "helper-hardlink", "helper-digest", "helper-size"],
)
def test_wasi_process_projection_joins_sdk_receipt_without_alias_relaxation(
    tmp_path, monkeypatch, mutation
):
    from tools.proof_queue_pkg import command_identity

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    sdk = installation.sdk
    plan = proof_plan.ProofPlan.load()
    env = {"WASI_SDK_PATH": str(sdk)}
    probes = []

    def version(argv, **kwargs):
        probes.append(tuple(argv))
        return subprocess.CompletedProcess(argv, 0, "clang version 23.1.0", "")

    monkeypatch.setattr(command_identity, "_run_captured", version)
    identity = command_identity._tool_identity(
        plan, "wasi-clang", {}, ["clang", "input.c"], cwd=tmp_path, env=env
    )
    command_identity._validate_toolchain_identity(plan, "wasi-clang", identity)
    closure = identity["wasi_sdk"]
    fact = closure["generation"]["facts"]["tools"]["clang"]
    raw = str(Path(closure["sdk"]) / fact["path"])
    expected_path = raw[0].lower() + raw[1:] if os.name == "nt" else raw
    assert identity["path"] == expected_path
    assert probes == [(expected_path, "--version")]
    assert closure["sdk"] == str(sdk)  # The managed SDK receipt is not rewritten.
    assert (
        process_image_capture.revalidate_images(identity["process_images"])
        == identity["process_images"]
    )
    if mutation == "none":
        return
    if mutation == "launcher-hardlink":
        alias = tmp_path / "other-clang.exe"
        os.link(identity["path"], alias)
        assert alias.samefile(identity["path"])
        identity["path"] = str(alias)
        expected = "compiler selection differs from its role"
    else:
        image = identity["process_images"][0]
        if mutation == "helper-hardlink":
            alias = tmp_path / "other-helper.exe"
            os.link(image["path"], alias)
            assert alias.samefile(image["path"])
            image["path"] = str(alias)
            expected = "process helper closure is incomplete"
        elif mutation == "helper-digest":
            image["sha256"] = "0" * 64
            expected = "process helper content differs from receipt"
        else:
            image["size_bytes"] += 1
            expected = "process helper content differs from receipt"
    with pytest.raises(ValueError, match=expected):
        toolchain_capture.validate_wasi_sdk_closure(
            identity, full_capture=False, selected_role="clang"
        )


@pytest.mark.skipif(
    os.name != "nt", reason="native Windows case-sensitive alias capability"
)
def test_case_distinct_target_alias_retains_resolved_content_role(
    tmp_path, monkeypatch
):
    resolved = _resolved(tmp_path)
    alias = resolved.tools.cc.path
    payload = alias.read_bytes()
    alias.unlink()
    content = alias.with_name(alias.name.upper())
    content.write_bytes(payload)
    try:
        alias.symlink_to(content)
    except OSError as exc:
        pytest.skip(f"case-distinct symlink capability unavailable: {exc}")
    assert {p.name for p in alias.parent.iterdir()} >= {alias.name, content.name}
    assert alias.samefile(content)
    identity = _capture(monkeypatch, resolved)
    rows = {row["role"]: row for row in provider.family_process_images(identity)}
    assert (
        rows["source-extension:cc"]["path"]
        != rows["source-extension:cc:content"]["path"]
    )
    assert rows["source-extension:cc:content"]["path"].endswith(content.name)
    assert (
        rows["source-extension:cc"]["sha256"]
        == rows["source-extension:cc:content"]["sha256"]
    )


def test_wasi_metadata_consumes_selected_archive_without_discovery(
    tmp_path, monkeypatch
):
    resolved = _resolved(tmp_path / "toolchain", "wasm")
    build_machine = _resolved(tmp_path / "build-machine", "native")
    monkeypatch.setattr(
        source_extension_toolchain,
        "_resolve_source_extension_native_toolchain",
        lambda *a, **kw: build_machine,
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
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
    recorded = metadata.payload["toolchain"]["link_probe_archives"]["compiler_rt"]
    assert recorded == {
        "path": str(resolved.link_inputs.compiler_rt),
        "sha256": resolved.link_inputs.sha256,
    }
    cross = metadata.meson_cross.read_text(encoding="utf-8")
    assert "c_link_args = []" in cross and "cpp_link_args = []" in cross
    assert "-nodefaultlibs" not in cross
    assert str(resolved.link_inputs.compiler_rt).replace("\\", "/") not in cross
    resolved.link_inputs.compiler_rt.write_bytes(b"changed archive")
    selected = capture_wasi_sdk_selection(
        root=proof_plan.ROOT, env={"WASI_SDK_PATH": str(resolved.wasi_c_abi.sdk)}
    )
    with pytest.raises(ValueError, match="provisioned generation"):
        toolchain_capture.capture_wasi_sdk_resources(selected)


@pytest.mark.parametrize("mismatch", ["archive", "sysroot", "plan"])
def test_wasi_metadata_refuses_different_selected_plan_before_effects(
    tmp_path, monkeypatch, mismatch
):
    resolved = _resolved(tmp_path / "selected", "wasm")
    foreign = _resolved(tmp_path / "foreign", "wasm")
    if mismatch == "archive":
        resolved = replace(resolved, link_inputs=foreign.link_inputs)
    elif mismatch == "sysroot":
        resolved = replace(resolved, wasi_sysroot=foreign.wasi_sysroot)
    else:
        resolved = replace(resolved, wasi_c_abi=None)
    monkeypatch.setattr(
        source_extension_toolchain,
        "_resolve_source_extension_native_toolchain",
        lambda *a, **kw: pytest.fail(
            "invalid captured C ABI reached build-machine resolution"
        ),
    )
    out_dir = tmp_path / "must-not-exist"
    with pytest.raises(ValueError, match="selected|C ABI"):
        source_extension_toolchain._materialize_source_extension_target_metadata_with_toolchain(
            molt_root=Path(__file__).resolve().parents[2],
            out_dir=out_dir,
            target_plan=resolved.target_plan,
            python_version="3.12",
            abi_tier="cpython-abi",
            toolchain=resolved,
            environment={},
        )
    assert not out_dir.exists()


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
    if resolved.wasi_c_abi is not None:
        environment["WASI_SDK_PATH"] = str(resolved.wasi_c_abi.sdk)
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
    install_module_view(
        monkeypatch, "subprocess", subprocess, source_extension_toolchain, run=forbidden
    )
    provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize(
    "triple, host_platform, host_arch",
    [
        ("x86_64-unknown-linux-gnu", "linux", "x86_64"),
        ("aarch64-apple-darwin", "darwin", "arm64"),
        ("x86_64-pc-windows-msvc", "win32", "AMD64"),
    ],
)
def test_recorded_native_target_is_independent_of_inspector_host(
    tmp_path, monkeypatch, triple, host_platform, host_arch
):
    monkeypatch.setattr(
        provider,
        "resolve_source_extension_target_plan",
        lambda requested: resolve_source_extension_target_plan(
            requested, host_platform=host_platform, host_arch=host_arch
        ),
    )
    identity = _capture(
        monkeypatch,
        _resolved(tmp_path, host_platform=host_platform, host_arch=host_arch),
    )
    assert identity["target"] == "native"
    assert identity["target_triple"] == triple

    def forbidden(*_args, **_kwargs):
        pytest.fail("recorded identity must not select from inspector host or probe")

    monkeypatch.setattr(provider, "resolve_source_extension_target_plan", forbidden)
    monkeypatch.setattr(provider, "_resolve_source_extension_toolchain", forbidden)
    monkeypatch.setattr(
        source_extension_toolchain.subprocess,
        "run",
        same_thread_probe(source_extension_toolchain.subprocess.run, forbidden),
    )
    provider.validate_identity(_policy(), identity)
    if triple.endswith("-msvc"):
        identity["commands"]["c"].append("--driver-mode=gcc")
        _reseal_command_change(identity, "c")
        with pytest.raises(ValueError, match="dialect gnu is incompatible"):
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
    # Actual image revalidation now owns byte drift before command validation.
    with pytest.raises(
        ValueError, match="process image changed while live custody armed"
    ) as caught:
        provider.validate_identity(_policy(), identity)
    assert process_image_capture._image_path_key(resolved.tools.cc.path) in str(
        caught.value
    )


@pytest.mark.parametrize("target", ["native", "wasm-freestanding"])
@pytest.mark.parametrize("role", ["c", "cpp"])
def test_resealed_foreign_compiler_target_is_rejected(
    tmp_path, monkeypatch, target, role
):
    identity = _capture(monkeypatch, _resolved(tmp_path, target))
    identity["commands"][role].append("--target=aarch64-unknown-linux-gnu")
    _reseal_command_change(identity, role)
    with pytest.raises(ValueError, match="target conflicts"):
        provider.validate_identity(_policy(), identity)


@pytest.mark.parametrize("flag", ["@unowned.rsp", "/clang:@unowned.rsp"])
def test_indirect_compiler_inputs_fail_closed(tmp_path, monkeypatch, flag):
    identity = _capture(monkeypatch, _resolved(tmp_path))
    identity["commands"]["c"].append(flag)
    _reseal_command_change(identity, "c")
    with pytest.raises(ValueError, match="custody"):
        provider.validate_identity(_policy(), identity)


def test_wasi_sysroot_bytes_are_identity_inputs(tmp_path, monkeypatch):
    resolved = _resolved(tmp_path, "wasm")
    identity = _capture(monkeypatch, resolved)
    (resolved.wasi_sysroot / "include" / "errno.h").write_text(
        "#define EINVAL 999\n", encoding="utf-8"
    )
    with pytest.raises(ValueError, match="provisioned generation"):
        toolchain_capture.capture_wasi_sdk_resources(identity["wasi_sdk"])


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
    _reseal_command_change(identity, "c")
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
        selected = images[0]["path"]
        for row in images:
            if row["path"] == selected:
                row["sha256"] = "0" * 64
        for phase in identity["native_compiler_phases"].values():
            for row in phase["phases"]:
                for helper in row["helpers"]:
                    if helper["path"] == selected:
                        helper["sha256"] = "0" * 64
    else:
        selected = images[0]["path"]
        for row in images:
            if row["path"] == selected:
                row["root_exit_disposition"] = "terminate"
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
        row["path"] == process_image_capture._image_path_key(alias)
        and row["path_kind"] == "selection"
        for row in images
    )
    assert any(
        row["path"] == process_image_capture._image_path_key(content.path)
        for row in images
    )
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

    install_module_view(
        monkeypatch, "subprocess", subprocess, source_extension_toolchain, run=run
    )
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


@pytest.mark.parametrize(
    "ambient_key", ["ProgramFiles", "LOCALAPPDATA", "HOME", "USERPROFILE"]
)
def test_wasi_readiness_never_adopts_ambient_header_tree(
    tmp_path, monkeypatch, ambient_key
):
    ambient = tmp_path / "ambient"
    (ambient / "wasi-sdk/share/wasi-sysroot/include").mkdir(parents=True)
    (ambient / "wasi-sdk/share/wasi-sysroot/include/errno.h").write_text(
        "#define EINVAL 22\n", encoding="utf-8"
    )
    monkeypatch.setenv(ambient_key, str(ambient))
    monkeypatch.setattr(
        llvm_toolchain,
        "provisioned_wasi_sdk_prefix",
        lambda *a, **kw: tmp_path / "absent",
    )
    assert (
        wasm_link_inputs.resolve_wasi_sysroot(env={ambient_key: str(ambient)}) is None
    )


@pytest.mark.parametrize("selector", ["MOLT_WASI_SYSROOT", "WASI_SYSROOT"])
def test_wasi_standalone_sysroot_cannot_replace_selected_sdk(tmp_path, selector):
    install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    foreign = tmp_path / "foreign"
    (foreign / "include").mkdir(parents=True)
    (foreign / "include/errno.h").write_text("#define EINVAL 22\n", encoding="utf-8")
    with pytest.raises(ValueError, match="differs|conflict"):
        wasm_link_inputs.resolve_wasi_sysroot(
            env={"WASI_SDK_PATH": str(install.sdk), selector: str(foreign)}
        )


def test_wasi_sdk_selection_is_fresh_and_uses_selected_home(tmp_path, monkeypatch):
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    for name in ("first", "second"):
        home = tmp_path / name
        home.mkdir()
        install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(home))
        selector = "~/" + install.sdk.relative_to(home).as_posix()
        assert (
            wasm_link_inputs.resolve_wasi_sysroot(
                env={home_key: str(home), "WASI_SDK_PATH": selector}
            )
            == install.sysroot
        )


@pytest.mark.parametrize("alteration", ["none", "crlf", "trailing-space"])
def test_sdk_fixture_receipt_keeps_exact_lf_generation_bytes(tmp_path, alteration):
    install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    receipt = install.prefix / ".molt-wasi-sdk.json"
    original = receipt.read_bytes()
    # The provisioner emits one compact UTF-8 JSON line, regardless of host.
    # Checking physical bytes detects Windows text-mode translation independently
    # of the JSON decoder, which deliberately accepts JSON whitespace.
    assert original.endswith(b"\n") and original.count(b"\n") == 1
    assert b"\r" not in original
    selected = llvm_toolchain.project_wasm_toolchain_environment(install, environ={})
    admitted = capture_wasi_sdk_selection(root=proof_plan.ROOT, env=selected)
    assert admitted["receipt"]["size_bytes"] == len(original)
    assert admitted["receipt"]["sha256"] == hashlib.sha256(original).hexdigest()
    if alteration == "none":
        assert receipt.read_bytes() == original
        return
    changed = (
        original[:-1] + b"\r\n" if alteration == "crlf" else original[:-1] + b" \n"
    )
    assert json.loads(changed) == json.loads(original)
    receipt.write_bytes(changed)
    with pytest.raises(ValueError, match="finite generation"):
        capture_wasi_sdk_selection(root=proof_plan.ROOT, env=selected)


@pytest.mark.parametrize("joined", [False, True])
def test_wasi_compiler_user_sysroot_uses_same_selected_path_for_probe_and_identity(
    tmp_path, monkeypatch, joined
):
    home = tmp_path / "selected home"
    home.mkdir()
    install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(home))
    home_key = "USERPROFILE" if os.name == "nt" else "HOME"
    selected = llvm_toolchain.project_wasm_toolchain_environment(
        install, environ={home_key: str(home)}
    )
    selected["MOLT_WASM_CC"] = (
        '"'
        + selected["CC_wasm32-wasip1"]
        + '" --sysroot'
        + ("=" if joined else " ")
        + '"~/'
        + install.sysroot.relative_to(home).as_posix()
        + '"'
    )
    monkeypatch.setenv(home_key, str(tmp_path / "ambient"))
    monkeypatch.setattr(
        llvm_wasi_tools, "_tool_version", lambda _path, **kw: install.asset.llvm_version
    )
    probes = []

    def probe(command, *, target_plan, environment):
        assert environment == selected
        assert compiler_sysroot_arg_value(command) == str(install.sysroot)
        probes.append(command)
        return None

    monkeypatch.setattr(
        source_extension_toolchain, "_probe_wasm_source_extension_compiler", probe
    )
    target = resolve_source_extension_target_plan("wasm")
    actual = source_extension_toolchain._resolve_source_extension_wasm_toolchain(
        target, environment=selected
    )
    assert actual.ok
    commands = source_extension_toolchain._source_extension_c_commands(
        toolchain=actual, target_plan=target, environment=selected
    )
    assert probes == []
    assert actual.wasi_sysroot == install.sysroot
    assert compiler_sysroot_arg_value(commands["c"]) == str(install.sysroot)
    assert compiler_sysroot_arg_value(commands["cpp"]) == str(install.sysroot)


def _owned_manifest(root):
    member = root / "include" / "errno.h"
    member.parent.mkdir(parents=True, exist_ok=True)
    member.write_bytes(b"/*owned*/\n")
    return {
        "root": str(root),
        "file_count": 1,
        "directories": ["include"],
        "files": [
            {
                "relative_path": "include/errno.h",
                "size": 10,
                "sha256": hashlib.sha256(b"/*owned*/\n").hexdigest(),
            }
        ],
        "manifest_sha256": "b" * 64,
    }


def test_owned_directory_projects_relative_members_without_a_second_manifest(tmp_path):
    manifest = _owned_manifest(tmp_path)
    files = toolchain_capture.frozen_files({"resource_custody": manifest})
    assert files == [
        toolchain_capture.FrozenFile(
            str(tmp_path / "include" / "errno.h"),
            hashlib.sha256(b"/*owned*/\n").hexdigest(),
            10,
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


def test_shared_sdk_armed_capture_and_cas_preserve_exact_family(tmp_path, monkeypatch):
    from tools.proof_queue_pkg import (
        command_identity,
        custody_cas,
        execution_environment,
    )

    resolved = _resolved(tmp_path, "wasm")
    derived = _capture(monkeypatch, resolved)
    abi = resolved.wasi_c_abi
    env = {"WASI_SDK_PATH": str(abi.sdk)}
    plan = proof_plan.ProofPlan.load()
    monkeypatch.setattr(
        command_identity,
        "_run_captured",
        lambda argv, **kwargs: subprocess.CompletedProcess(
            argv, 0, "clang version 23.1.0", ""
        ),
    )
    static = command_identity._tool_identity(
        plan, "wasi-clang", {}, [str(abi.driver)], cwd=tmp_path, env=env
    )
    located = {"wasi-clang": static, "source-extension": derived}
    monkeypatch.setattr(
        command_identity, "_python_identity", lambda *args, **kwargs: None
    )
    from molt import wasi_sdk_identity

    tree_calls = []
    tree_identity = wasi_sdk_identity.wasi_sdk_tree_identity

    def tree(path):
        tree_calls.append(path)
        return tree_identity(path)

    monkeypatch.setattr(wasi_sdk_identity, "wasi_sdk_tree_identity", tree)
    image_calls = []
    capture_file = wasi_sdk_identity.stable_regular_file_identity

    def image(path, **kwargs):
        image_calls.append(path.resolve())
        return capture_file(path, **kwargs)

    monkeypatch.setattr(wasi_sdk_identity, "stable_regular_file_identity", image)
    _, captured = execution_environment._capture_toolchains(
        {"toolchains": list(located)},
        [str(abi.driver)],
        cwd=tmp_path,
        env=env,
        source_root=proof_plan.ROOT,
        hash_workers=1,
        located_toolchains=located,
    )
    assert tree_calls == [abi.sdk / "lib", abi.sysroot]
    assert len(image_calls) == len(set(image_calls)) == 5
    assert (
        captured["wasi-clang"]["wasi_sdk"] is captured["source-extension"]["wasi_sdk"]
    )
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(cas, captured)
    raw = custody_cas.read_ref(reference, expected_root=cas)
    first = raw["toolchains"]["wasi-clang"]["wasi_sdk"]["resources"]
    second = raw["toolchains"]["source-extension"]["wasi_sdk"]["resources"]
    assert set(first) == {"artifact"} and first == second
    reads = []
    read_ref = custody_cas.read_ref

    def read(ref, **kwargs):
        reads.append(ref)
        return read_ref(ref, **kwargs)

    monkeypatch.setattr(custody_cas, "read_ref", read)
    expanded = toolchain_capture.load_capture(reference, cas_root=cas)
    assert reads.count(first["artifact"]) == 1
    assert expanded["files"] == [
        row.as_dict() for row in toolchain_capture.frozen_files(expanded["toolchains"])
    ]
    (abi.include / "errno.h").write_bytes(b"changed under proof custody")
    checked = toolchain_capture.verify_capture(reference, workers=1, cas_root=cas)
    assert not checked["stable"]
    assert abi.include / "errno.h" in {
        Path(row["path"]) for row in checked["mismatches"]
    }


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-closure",
        "missing-target-and-closure",
        "missing-resource",
        "foreign-root",
        "missing-image",
        "missing-file",
    ],
)
def test_sdk_full_capture_receiver_rejects_resealed_omissions(
    tmp_path, monkeypatch, mutation
):
    import copy
    from tools.proof_queue_pkg import custody_cas

    resolved = _resolved(tmp_path, "wasm")
    identity = _capture(monkeypatch, resolved)
    identity["wasi_sdk"] = toolchain_capture.capture_wasi_sdk_resources(
        identity["wasi_sdk"]
    )
    _rehash(identity)
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(
        cas, {"source-extension": identity}
    )
    if mutation == "missing-file":
        raw = custody_cas.read_ref(reference, expected_root=cas)
        raw["files"].pop()
        reference = custody_cas.put_json(cas, raw).as_dict()
        with pytest.raises(ValueError, match="frozen file"):
            toolchain_capture.load_capture(reference, cas_root=cas)
        return
    changed = copy.deepcopy(identity)
    if mutation == "missing-closure":
        del changed["wasi_sdk"]
    elif mutation == "missing-target-and-closure":
        del changed["wasi_sdk"]
        del changed["target_triple"]
    elif mutation == "missing-resource":
        changed["wasi_sdk"]["resources"].pop()
    elif mutation == "foreign-root":
        # Keep every byte real so the independently resealed receiver reaches
        # SDK ownership validation, including Windows image-coordinate lookup.
        resource = changed["wasi_sdk"]["resources"][0]
        foreign = tmp_path / "foreign-sdk-resource"
        shutil.copytree(Path(resource["root"]), foreign)
        resource["root"] = str(foreign)
    else:
        changed["process_images"].pop()
    _rehash(changed)
    with pytest.raises(ValueError):
        toolchain_capture.publish_capture(cas, {"source-extension": changed})
    # A writer can reseal an invalid record without calling publish_capture.
    # The persisted receiver must enforce the same obligation independently.
    raw = custody_cas.read_ref(reference, expected_root=cas)
    raw["toolchains"] = toolchain_capture._store_sdk_resources(
        cas, {"source-extension": changed}
    )
    raw["files"] = [row.as_dict() for row in toolchain_capture.frozen_files(changed)]
    invalid = custody_cas.put_json(cas, raw).as_dict()
    with pytest.raises(ValueError):
        toolchain_capture.load_capture(invalid, cas_root=cas)


@pytest.mark.parametrize(
    "mutation",
    ["missing-phases", "missing-language", "missing-driver", "missing-transcript"],
)
def test_native_source_extension_receivers_require_compiler_phases(
    tmp_path, monkeypatch, mutation
):
    import copy
    from tools.proof_queue_pkg import custody_cas

    identity = _capture(monkeypatch, _resolved(tmp_path))
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(
        cas, {"source-extension": identity}
    )
    changed = copy.deepcopy(identity)
    if mutation == "missing-phases":
        del changed["native_compiler_phases"]
    elif mutation == "missing-language":
        changed["native_compiler_phases"].pop("cpp")
    elif mutation == "missing-transcript":
        changed["native_compiler_phases"]["c"]["probes"].pop()
    else:
        changed["process_images"] = [
            row
            for row in changed["process_images"]
            if row["role"] != "source-extension-phase:c"
        ]
    _rehash(changed)
    with pytest.raises(ValueError):
        toolchain_capture.publish_capture(cas, {"source-extension": changed})
    raw = custody_cas.read_ref(reference, expected_root=cas)
    raw["toolchains"] = {"source-extension": changed}
    raw["files"] = [row.as_dict() for row in toolchain_capture.frozen_files(changed)]
    forged = custody_cas.put_json(cas, raw).as_dict()
    with pytest.raises(ValueError):
        toolchain_capture.load_capture(forged, cas_root=cas)


@pytest.mark.parametrize("name", ["ordinary", "OneDrive - selected tools"])
def test_entrypoint_admission_uses_actual_identity_not_directory_name(
    tmp_path, monkeypatch, name
):
    resolved = _resolved(tmp_path / name)
    selected = resolved.tools.cc.path
    admitted = provider._entrypoint(str(selected), role="compiler")
    assert admitted.samefile(selected)
    traversal = selected.parent / ".." / selected.parent.name / selected.name
    # Product selection keeps the kernel's spelling; proof image/watch custody
    # is the owner that refuses unsupported traversal before hashing an image.
    lexical = provider._entrypoint(str(traversal), role="compiler")
    assert str(lexical) == str(traversal)
    assert lexical.samefile(selected)
    tools = resolved.tools.metadata()
    roles, _ = _source_extension_tool_role_contract(resolved.target_plan.target_triple)
    compiler = tools[roles["c"]]
    compiler["path"] = str(traversal)
    compiler["command"][0] = str(traversal)
    monkeypatch.setattr(
        provider,
        "native_executable_content_identity",
        lambda *args, **kwargs: pytest.fail("unadmitted compiler image hashed"),
    )
    with pytest.raises(ValueError, match="parent traversal"):
        provider._tool_images(tools, target=resolved.target_plan)

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path
from types import MappingProxyType

import pytest

from molt.cli import runtime_cargo_plan as plans
from molt.cli import runtime_fingerprints as fingerprints
from molt.cli.runtime_build_identity import (
    _runtime_build_environment_identity,
    _verify_plan_toolchain_content,
)
from molt.cli.runtime_cargo_plan import _rust_resource_roots
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    runtime_build_fingerprint,
)
from molt.exact_json import canonical_json_sha256
from tests.runtime_build_identity_helper import (
    native_runtime_staticlib_identity,
    runtime_build_identity,
)


def _reseal(value: dict) -> None:
    family = value["payload"]["family"]
    family["compile_digest"] = canonical_json_sha256(family["compile"])
    value["compile_digest"] = family["compile_digest"]
    value["family_digest"] = canonical_json_sha256(family)
    value["digest"] = canonical_json_sha256(value["payload"])


@pytest.mark.parametrize("field", ["sources", "toolchain", "common_config"])
def test_resealed_empty_compile_subauthorities_are_rejected(field: str) -> None:
    value = runtime_build_identity("shared").to_dict()
    value["payload"]["family"]["compile"][field] = {}
    _reseal(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity(
            value["digest"],
            value["compile_digest"],
            value["family_digest"],
            value["payload"],
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "unknown-tool",
        "missing-archive",
        "bool-size",
        "foreign-path",
        "wrong-target",
        "unknown-config",
        "missing-python",
        "unknown-wrapper",
    ],
)
def test_resealed_toolchain_and_config_drift_is_rejected(mutation: str) -> None:
    value = runtime_build_identity("shared").to_dict()
    compile_payload = value["payload"]["family"]["compile"]
    toolchain = compile_payload["toolchain"]
    if mutation == "unknown-tool":
        toolchain["tools"]["unknown"] = toolchain["tools"]["rustc"]
    elif mutation == "missing-archive":
        toolchain["archives"].pop()
    elif mutation == "bool-size":
        toolchain["tools"]["rustc"]["size"] = True
    elif mutation == "foreign-path":
        toolchain["tools"]["rustc"]["entrypoint"] = "C:\\outside\\rustc.exe"
    elif mutation == "wrong-target":
        compile_payload["common_config"]["target_triple"] = "aarch64-apple-darwin"
    elif mutation == "unknown-config":
        compile_payload["common_config"]["legacy"] = True
    elif mutation == "missing-python":
        del toolchain["tools"]["build_python"]
    else:
        toolchain["wrappers"]["UNRECOGNIZED"] = toolchain["tools"]["rustc"]
    _reseal(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(value)


def test_publication_change_reuses_only_compile_and_member_output() -> None:
    before = runtime_build_identity(
        "shared", "old-publication", compile_seed="same-compile"
    )
    after = runtime_build_identity(
        "shared", "new-publication", compile_seed="same-compile"
    )
    assert before.compile_digest == after.compile_digest
    assert before.family_digest != after.family_digest
    assert (
        runtime_build_fingerprint(before)["hash"]
        != runtime_build_fingerprint(after)["hash"]
    )
    for scope in ("compile", "member-output"):
        first = runtime_build_fingerprint(before, scope=scope)
        second = runtime_build_fingerprint(after, scope=scope)
        assert first["hash"] == second["hash"]
        assert fingerprints._runtime_fingerprint_payload_is_valid(
            {"version": 3, **first}
        )


@pytest.mark.parametrize("scope", ["compile", "member", "member-output"])
def test_fingerprint_is_a_checked_projection_not_an_independent_claim(
    scope: str,
) -> None:
    identity = native_runtime_staticlib_identity()
    payload = {"version": 3, **runtime_build_fingerprint(identity, scope=scope)}
    assert fingerprints._runtime_fingerprint_payload_is_valid(payload)
    payload["hash"] = "f" * 64
    assert not fingerprints._runtime_fingerprint_payload_is_valid(payload)


@pytest.mark.parametrize("scope", [[], {}, 1, None, "unrecognized"])
def test_untrusted_fingerprint_scope_fails_without_type_leak(scope: object) -> None:
    payload = {
        "version": 3,
        **runtime_build_fingerprint(native_runtime_staticlib_identity()),
    }
    payload["build_identity_scope"] = scope
    assert not fingerprints._runtime_fingerprint_payload_is_valid(payload)


def test_fingerprint_reader_rejects_duplicate_keys_and_restored_mtime(
    tmp_path: Path,
) -> None:
    path = tmp_path / "identity.json"
    payload = {
        "version": 3,
        **runtime_build_fingerprint(native_runtime_staticlib_identity()),
    }
    original = json.dumps(payload)
    path.write_text(original, encoding="utf-8")
    assert fingerprints._read_runtime_fingerprint(path) == payload
    before = path.stat()
    path.write_text(original.replace('"version": 3', '"version": 2'), encoding="utf-8")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert fingerprints._read_runtime_fingerprint(path) is None
    path.write_text('{"version":3,' + original[1:], encoding="utf-8")
    assert fingerprints._read_runtime_fingerprint(path) is None


@pytest.fixture
def plan_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    def executable(value: str, **_kwargs: object) -> Path:
        if Path(value).is_absolute() and Path(value).is_file():
            return Path(value)
        path = tmp_path / str(value).replace("/", "_").replace("\\", "_")
        if not path.exists():
            path.write_bytes(b"MZ" + str(value).encode())
        return path

    monkeypatch.setattr(plans, "resolve_executable", executable)
    monkeypatch.setattr(
        plans.shutil, "which", lambda value, **kwargs: str(tmp_path / value)
    )
    resources = tmp_path / "rust-resources"
    resources.mkdir()
    (resources / "libcore.rlib").write_bytes(b"rust-core")
    monkeypatch.setattr(
        plans,
        "_rust_resource_roots",
        lambda *args, **kwargs: (plans.CargoResourceRoot("rust/test", resources),),
    )
    return tmp_path


def _metadata_stdout(sysroot: Path, cfg: str = "unix\nselected\n") -> str:
    return "\n".join(
        (
            "___",
            "lib___.rlib",
            "lib___.so",
            "lib___.so",
            "lib___.a",
            "lib___.so",
            str(sysroot),
            "off",
            "___",
            cfg,
        )
    )


def _metadata(sysroot: Path, cfg: str = "unix\nselected\n"):
    from molt.cli.cargo_target_cfg import parse_rustc_target_metadata

    return parse_rustc_target_metadata(_metadata_stdout(sysroot, cfg), "")


def _plan(
    root: Path,
    *,
    env: dict[str, str] | None = None,
    args: tuple[str, ...] = (),
    target: str | None = None,
    **kwargs: object,
) -> plans.RuntimeCargoPlan:
    return plans.resolve_runtime_cargo_plan(
        root,
        env={"CARGO_HOME": str(root / "cargo-home"), **(env or {})},
        cargo_command=("cargo", "rustc", *args),
        requested_target=target,
        host_target="x86_64-unknown-linux-gnu",
        **kwargs,
    )


def _config(root: Path, text: str) -> Path:
    directory = root / ".cargo"
    directory.mkdir(exist_ok=True)
    path = directory / "config.toml"
    path.write_text(text, encoding="utf-8")
    return path


def test_environment_linker_overrides_config_and_cli_overrides_environment(
    plan_root: Path,
) -> None:
    _config(
        plan_root, '[target.x86_64-unknown-linux-gnu]\nlinker="configured-linker"\n'
    )
    environment = {"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "environment-linker"}
    assert (
        _plan(plan_root, env=environment).tools["linker"].name == "environment-linker"
    )
    explicit = _plan(
        plan_root,
        env=environment,
        args=("--config", 'target.x86_64-unknown-linux-gnu.linker="cli-linker"'),
    )
    assert explicit.tools["linker"].name == "cli-linker"
    assert explicit.environment["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"] == str(
        explicit.tools["linker"]
    )


@pytest.mark.parametrize(
    "target",
    [
        "wasm32-wasip1",
        "wasm32-unknown-unknown",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ],
)
def test_cfg_target_plan_uses_rustc_facts_and_pins_selected_tools(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    target: str,
) -> None:
    wasm = target.startswith("wasm32")
    fact_text = 'target_arch="wasm32"' if wasm else 'target_arch="native-test"'
    seen = []

    def probe(rustc, selected_target, flags, **kwargs):
        seen.append((selected_target, flags))
        return _metadata(plan_root, fact_text)

    monkeypatch.setattr(plans, "_rust_target_metadata", probe)
    _config(
        plan_root,
        "[target.'cfg(not(target_arch=\"wasm32\"))']\nrustflags=[]\n"
        '[target.\'cfg(target_arch="wasm32")\']\nlinker="wasm-linker"\nrustflags=["--cfg","wasm_selected"]\n'
        '[build]\nrustflags=["--cfg","baseline"]\n',
    )
    plan = _plan(
        plan_root,
        args=("--target", target),
        target=target,
        env={name: "selected-" + name.lower() for name in ("CC", "CXX", "AR", "RANLIB")}
        | (
            {}
            if wasm or target == "x86_64-unknown-linux-gnu"
            else {
                f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER": "cross-linker"
            }
        ),
    )
    assert plan.rustflags == ("--cfg", "wasm_selected" if wasm else "baseline")
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert seen and all(selected == target for selected, _ in seen)
    if wasm:
        assert plan.tools["linker"].name == "wasm-linker"
        assert any("wasm-linker" in token for token in plan.command)


@pytest.mark.parametrize("failure", ["exit", "empty", "mutated", "none"])
def test_cfg_probe_retains_selected_compiler_and_reports_failure(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    rustc = plan_root / "cfg-rustc"
    rustc.write_bytes(b"compiler-before")
    expected = [
        str(rustc),
        *plans.cargo_target_query_arguments("wasm32-wasip1", ("--cfg", "selected")),
    ]
    seen = []

    def run(command, **kwargs):
        seen.append(command)
        assert kwargs["cwd"] == plan_root
        assert kwargs["timeout"] == 30
        assert kwargs["input"] == ""
        assert "RUSTC_LOG" not in kwargs["env"]
        if failure == "mutated":
            rustc.write_bytes(b"compiler-after!")
        return subprocess.CompletedProcess(
            command,
            7 if failure == "exit" else 0,
            ""
            if failure == "empty"
            else _metadata_stdout(plan_root, 'target_arch="wasm32"\nselected\n'),
            "cfg diagnostic" if failure == "exit" else "",
        )

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    if failure == "none":
        metadata = plans._rust_target_metadata(
            rustc,
            "wasm32-wasip1",
            ("--cfg", "selected"),
            root=plan_root,
            env={"RUSTC_LOG": "debug"},
        )
        assert ("target_arch", "wasm32") in metadata.cfg
        assert ("selected", None) in metadata.cfg
    else:
        with pytest.raises((ValueError, OSError)) as exc:
            plans._rust_target_metadata(
                rustc,
                "wasm32-wasip1",
                ("--cfg", "selected"),
                root=plan_root,
                env={},
            )
        if failure == "exit":
            assert "cfg diagnostic" in str(exc.value)
    assert seen == [expected]


def test_cfg_linker_conflicts_are_rejected_unless_exact_target_wins(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        plans, "_rust_target_metadata", lambda *a, **k: _metadata(plan_root, "unix")
    )
    _config(
        plan_root,
        "[target.'cfg(unix)']\nlinker=\"first\"\n"
        "[target.'cfg(all(unix))']\nlinker=\"second\"\n",
    )
    with pytest.raises(ValueError, match="multiple cfg linkers"):
        _plan(plan_root)
    plan = _plan(
        plan_root, env={"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "exact"}
    )
    assert plan.tools["linker"].name == "exact"


def test_empty_wrapper_environment_disables_configured_wrapper(plan_root: Path) -> None:
    _config(plan_root, '[build]\nrustc-wrapper="configured-wrapper"\n')
    plan = _plan(plan_root, env={"RUSTC_WRAPPER": ""})
    assert not plan.wrappers
    assert plan.environment["RUSTC_WRAPPER"] == ""


@pytest.mark.parametrize("scope", ["build", "target.x86_64-unknown-linux-gnu"])
@pytest.mark.parametrize("override", [None, "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"])
def test_cargo_string_lists_merge_file_cli_and_environment_before_cfg_selection(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, scope: str, override: str | None
) -> None:
    _config(
        plan_root,
        f'[{scope}]\nrustflags=["--cfg","file"]\n'
        "[target.'cfg(all(file,cli,environment))']\nlinker=\"merged-linker\"\n",
    )
    selected_environment = (
        "CARGO_BUILD_RUSTFLAGS"
        if scope == "build"
        else "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS"
    )
    env = {selected_environment: "--cfg environment"}
    expected = ("--cfg", "file", "--cfg", "cli", "--cfg", "environment")
    if override is not None:
        env[override] = "--cfg override" if override == "RUSTFLAGS" else ""
        expected = ("--cfg", "override") if override == "RUSTFLAGS" else ()
    probes = []

    def metadata(rustc, target, flags, **kwargs):
        probes.append((target, flags))
        return _metadata(plan_root, "\n".join(("unix", *flags[1::2])))

    monkeypatch.setattr(plans, "_rust_target_metadata", metadata)
    plan = _plan(
        plan_root, env=env, args=("--config", f'{scope}.rustflags=["--cfg","cli"]')
    )
    assert plan.rustflags == expected
    assert probes == [(None, expected)]
    if override is None:
        assert plan.tools["linker"].name == "merged-linker"
    else:
        assert plan.tools["linker"].name != "merged-linker"


def test_encoded_flags_win_and_transform_is_applied_once(plan_root: Path) -> None:
    _config(plan_root, '[build]\nrustflags=["--cfg", "configured"]\n')
    seen = []

    def transform(flags: tuple[str, ...]) -> tuple[str, ...]:
        seen.append(flags)
        return (*flags, "--cfg", "molt")

    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "--cfg\x1fencoded",
            "RUSTFLAGS": "--cfg ignored",
        },
        rustflags_transform=transform,
    )
    assert seen == [("--cfg", "encoded")]
    assert plan.rustflags == ("--cfg", "encoded", "--cfg", "molt")
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)


def test_normalized_cfg_metadata_is_reused_for_dependency_and_final_resources(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    host = "x86_64-unknown-linux-gnu"
    installed, dependency, final = (
        plan_root / name for name in ("installed", "dependency", "final")
    )
    for sysroot in (installed, dependency, final):
        libdir = sysroot / "lib" / "rustlib" / host / "lib"
        libdir.mkdir(parents=True)
        (libdir / "libcore.rlib").write_bytes(sysroot.name.encode())
    (installed / "lib" / "rustc_driver.dll").write_bytes(b"installed-driver")
    (installed / "bin").mkdir()
    search = plan_root / "search"
    search.mkdir()
    original = ("-L", "native=search")
    normalized = ("-L", "native=" + str(search))
    final_flags = (*normalized, "--cfg", "final_crate")
    _config(plan_root, "[target.'cfg(normalized)']\nlinker=\"normalized-linker\"\n")
    probes = []

    def metadata(rustc, target, flags, **kwargs):
        wrapped = bool(kwargs["wrappers"])
        probes.append((target, flags, wrapped))
        if not wrapped:
            return _metadata(installed, "unix")
        selected = final if flags == final_flags else dependency
        facts = "unix\nnormalized" if flags[:2] == normalized else "unix"
        return _metadata(selected, facts)

    monkeypatch.setattr(plans, "_rust_target_metadata", metadata)
    monkeypatch.setattr(plans, "_rust_resource_roots", _rust_resource_roots)
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-L native=search", "RUSTC_WRAPPER": "metadata-wrapper"},
        args=("--", "--cfg", "final_crate"),
    )
    assert plan.rustflags == normalized
    assert plan.tools["linker"].name == "normalized-linker"
    # Resource discovery reuses the normalized cfg probe, while retaining a
    # distinct final-crate selection and the unwrapped installed driver.
    assert probes == [
        (None, original, True),
        (None, normalized, True),
        (None, (), False),
        (None, final_flags, True),
    ]
    roots = {entry.label: entry.path for entry in plan.rust_resources.roots}
    assert (
        roots["rust/target-libdir/0"] == dependency / "lib" / "rustlib" / host / "lib"
    )
    assert roots["rust/target-libdir/1"] == final / "lib" / "rustlib" / host / "lib"
    assert roots["rust/driver"] == installed / "lib"
    assert any(
        item.identity.path == installed / "lib" / "rustc_driver.dll"
        for item in plan.rust_resources.files
    )


def test_actual_host_target_flags_are_resolved_for_native(plan_root: Path) -> None:
    plan = _plan(
        plan_root, env={"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "--cfg host"}
    )
    assert plan.rustflags == ("--cfg", "host")


def test_configuration_parse_and_receipt_share_one_generation(plan_root: Path) -> None:
    path = _config(plan_root, '[build]\nrustflags="--cfg before"\n')
    plan = _plan(plan_root)
    before = path.stat()
    path.write_text('[build]\nrustflags="--cfg after!"\n', encoding="utf-8")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.configuration_identity()


def test_new_configuration_file_invalidates_live_plan(plan_root: Path) -> None:
    plan = _plan(plan_root)
    _config(plan_root, '[build]\nrustflags="--cfg inserted"\n')
    with pytest.raises(ValueError, match="selection changed"):
        plan.verify()


def test_compile_partition_excludes_only_link_arguments(plan_root: Path) -> None:
    (plan_root / "first.rsp").write_text("--export=first\n", encoding="utf-8")
    (plan_root / "second.rsp").write_text("--export=second\n", encoding="utf-8")
    first = _plan(
        plan_root, args=("--", "-C", "panic=abort", "-C", "link-arg=@first.rsp")
    )
    second = _plan(
        plan_root, args=("--", "-C", "panic=abort", "-C", "link-arg=@second.rsp")
    )
    assert first.partition_command()[0] == second.partition_command()[0]
    assert first.partition_command()[1] != second.partition_command()[1]
    assert "panic=abort" in first.partition_command()[0]
    assert (
        first.rust_resources.content_identity()
        == second.rust_resources.content_identity()
    )
    assert first.configuration_identity() == second.configuration_identity()
    assert first.project_link_arguments(
        first.partition_command()[1]
    ) != second.project_link_arguments(second.partition_command()[1])


def test_final_link_response_generation_is_fenced_even_after_byte_restore(
    plan_root: Path,
) -> None:
    response = plan_root / "exports.rsp"
    original = b"--export=first\n"
    response.write_bytes(original)
    plan = _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))
    before = response.stat()
    response.write_bytes(b"--export=other\n")
    response.write_bytes(original)
    os.utime(response, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()
    with pytest.raises(ValueError, match="changed"):
        plan.project_link_arguments(plan.partition_command()[1])


def test_link_response_projection_consumes_capture_not_a_second_read(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    response = plan_root / "exports.rsp"
    response.write_bytes(b"--export=first\n")
    plan = _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))
    monkeypatch.setattr(
        plans,
        "stable_regular_file_identity",
        lambda *args, **kwargs: pytest.fail(
            "response bytes were independently recaptured"
        ),
    )
    projected = plan.project_link_arguments(plan.partition_command()[1])
    assert any("@response:sha256=" in item for item in projected)


@pytest.mark.parametrize(
    "force,expected", [(False, "ambient-cc"), (True, "configured-cc")]
)
def test_cargo_environment_force_selects_and_pins_actual_build_tool(
    plan_root: Path, force: bool, expected: str
) -> None:
    _config(
        plan_root,
        '[env]\nCC={value="configured-cc",force=' + str(force).lower() + "}\n",
    )
    plan = _plan(plan_root, env={"CC": "ambient-cc"})
    assert plan.tools["cc"].name == expected
    assert plan.environment["CC_x86_64-unknown-linux-gnu"] == str(plan.tools["cc"])
    assert ('env."CC".force=false' in plan.command) is force


def test_cargo_environment_cli_value_origin_and_relative_resource_custody(
    plan_root: Path,
) -> None:
    resource = plan_root / "headers"
    resource.mkdir()
    header = resource / "api.h"
    header.write_bytes(b"header")
    _config(plan_root, '[env]\nSDK={value="unused",relative=true,force=true}\n')
    plan = _plan(plan_root, args=("--config", 'env.SDK.value="headers"'))
    assert plan.environment["SDK"] == str(resource)
    assert 'env."SDK".force=false' in plan.command
    assert any(item.identity.path == header for item in plan.rust_resources.files)
    header.write_bytes(b"changed header")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_nonforced_cargo_environment_does_not_capture_ignored_relative_path(
    plan_root: Path,
) -> None:
    _config(plan_root, '[env]\nSDK={value="absent",relative=true}\n')
    plan = _plan(plan_root, env={"SDK": "ambient"})
    assert plan.environment["SDK"] == "ambient"
    assert not any(
        item.label.startswith("cargo/env/") for item in plan.rust_resources.files
    )


@pytest.mark.parametrize("case_insensitive", [False, True])
def test_environment_key_semantics_are_platform_gated(case_insensitive: bool) -> None:
    env = plans._CargoEnvironment(
        {"cc": "lower", "CC": "upper"}, case_insensitive=case_insensitive
    )
    assert env["cc"] == ("upper" if case_insensitive else "lower")
    assert len(env) == (1 if case_insensitive else 2)
    selected, forced, _roots = plans._apply_cargo_environment(
        {"env": {"cC": {"value": "configured", "force": True}}}, {}, env, {}
    )
    assert selected["CC"] == ("configured" if case_insensitive else "upper")
    assert forced == ("cC",)


def test_windows_cargo_environment_case_alias_cannot_bypass_ambient_precedence() -> (
    None
):
    env = plans._CargoEnvironment({"CC": "ambient"}, case_insensitive=True)
    selected, forced, _roots = plans._apply_cargo_environment(
        {"env": {"cc": "configured"}}, {}, env, {}
    )
    assert selected["cc"] == "ambient"
    assert not forced
    with pytest.raises(ValueError, match="case-ambiguous"):
        plans._apply_cargo_environment({"env": {"cc": "one", "CC": "two"}}, {}, env, {})


def test_capture_hook_observes_final_selected_inputs_once(plan_root: Path) -> None:
    _config(plan_root, '[env]\nCC={value="configured-cc",force=true}\n')
    captures = []

    def capture(environment, tools, rust_roots):
        captures.append((dict(environment), dict(tools), rust_roots))
        with pytest.raises(TypeError):
            environment["CC"] = "other"

    plan = _plan(plan_root, capture_inputs=capture)
    assert len(captures) == 1
    environment, tools, roots = captures[0]
    assert environment == dict(plan.environment)
    assert tools == dict(plan.tools)
    assert tools["cc"].name == "configured-cc"
    assert roots == plan.rust_resources.roots


@pytest.mark.parametrize(
    "payload,diagnostic",
    [
        (b"@nested.rsp\n", "nests an unsupported response resource"),
        (b"--script=link.ld\n", "requires explicit resource custody"),
        (b"/LIBPATH:elsewhere\n", "requires explicit resource custody"),
        (b"library.a\n", "requires explicit resource custody"),
        (b"--export=has space\n", "unsafe whitespace"),
        (b"\xff\n", "not UTF-8"),
    ],
)
def test_unadmitted_link_response_resources_fail_before_execution(
    plan_root: Path, payload: bytes, diagnostic: str
) -> None:
    response = plan_root / "exports.rsp"
    response.write_bytes(payload)
    with pytest.raises(ValueError, match=diagnostic):
        _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))


def test_custom_profile_debug_policy_comes_from_inheritance_not_name(
    plan_root: Path,
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.futuristic]\ninherits="dev"\n[profile.debug_named]\ninherits="release"\n',
        encoding="utf-8",
    )
    plan = _plan(plan_root)
    assert plan.preserve_debug_for_profile("futuristic")
    assert not plan.preserve_debug_for_profile("debug_named")
    override = _plan(plan_root, env={"CARGO_PROFILE_FUTURISTIC_DEBUG": "0"})
    assert not override.preserve_debug_for_profile("futuristic")


@pytest.mark.parametrize(
    "role",
    [
        "cargo",
        "rustc",
        "cc",
        "cxx",
        "ar",
        "ranlib",
        "linker",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ],
)
def test_live_plan_fences_every_selected_tool_generation(
    plan_root: Path, role: str
) -> None:
    plan = _plan(
        plan_root,
        env={
            "RUSTC_WRAPPER": "wrapper",
            "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        },
    )
    path = (plan.wrappers if role in plan.wrappers else plan.tools)[role]
    before = path.stat()
    path.write_bytes(b"X" * before.st_size)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["same-size", "add", "remove"])
def test_live_plan_fences_resource_generation_and_membership(
    plan_root: Path, mutation: str
) -> None:
    plan = _plan(plan_root)
    resource = plan_root / "rust-resources" / "libcore.rlib"
    if mutation == "same-size":
        before = resource.stat()
        resource.write_bytes(b"X" * before.st_size)
        os.utime(resource, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif mutation == "remove":
        resource.unlink()
    else:
        resource.with_name("new-codegen.dll").write_bytes(b"new")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_inherited_profile_overrides_enter_identity_and_policy(plan_root: Path) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n', encoding="utf-8"
    )
    baseline = _plan(plan_root)
    inherited = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_LTO": "fat",
            "CARGO_PROFILE_RELEASE_DEBUG": "2",
            "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL": "3",
        },
    )
    before = _runtime_build_environment_identity(
        baseline, cargo_profile="release-output"
    )
    after = _runtime_build_environment_identity(
        inherited, cargo_profile="release-output"
    )
    assert before != after
    assert "CARGO_PROFILE_RELEASE_LTO" in after
    assert "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL" in after
    assert inherited.preserve_debug_for_profile("release-output")
    assert not baseline.preserve_debug_for_profile("release-output")


def test_profile_cli_inheritance_and_overlapping_prefixes_share_authority(
    plan_root: Path,
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n', encoding="utf-8"
    )
    plan = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "0",
            "CARGO_PROFILE_DEV_LTO": "thin",
        },
        args=("--config", 'profile.release-output.inherits="dev"'),
    )
    assert plan.profile_ancestry("release-output") == ("release-output", "dev")
    assert not plan.preserve_debug_for_profile("release-output")
    assert set(plan.profile_environment("release-output")) == {
        "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG",
        "CARGO_PROFILE_DEV_LTO",
    }


def test_unknown_inherited_profile_control_is_diagnosed(plan_root: Path) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.custom]\ninherits="release"\n', encoding="utf-8"
    )
    with pytest.raises(ValueError, match="unsupported output-bearing"):
        _plan(
            plan_root, env={"CARGO_PROFILE_RELEASE_FUTURE_CONTROL": "1"}
        ).profile_environment("custom")


def test_cli_tool_selectors_are_pinned_after_original_overrides(
    plan_root: Path,
) -> None:
    plan = _plan(
        plan_root,
        args=(
            "--config",
            'build.rustc="configured-rustc"',
            "--config",
            'build.rustc-wrapper="configured-wrapper"',
            "--",
            "-C",
            "panic=abort",
        ),
    )
    selected = plans._command_options(plan.command)[1]
    pinned = plans._pinned_tool_configurations(plan.tools, plan.wrappers, plan.target)
    assert selected[-len(pinned) :] == tuple(raw for raw, _ in pinned)
    assert plan.command[-3:] == ("--", "-C", "panic=abort")
    # Host absolute paths are live execution inputs, not portable receipt data.
    assert str(plan_root) not in " ".join(plan.partition_command()[0])
    assert plan.configuration_arguments()[-len(pinned) :] == tuple(
        canonical_json_sha256(plans.tomllib.loads(logical)) for _, logical in pinned
    )


def test_rustup_proxy_is_pinned_but_custom_compiler_is_preserved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    suffix = ".exe" if os.name == "nt" else ""
    proxy = tmp_path / ("rustup" + suffix)
    selector = tmp_path / ("rustc" + suffix)
    compiler = tmp_path / ("selected-rustc" + suffix)
    proxy.write_bytes(b"MZrustup")
    selector.write_bytes(proxy.read_bytes())
    compiler.write_bytes(b"MZcompiler")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(
            command, 0, stdout=str(compiler) + "\n", stderr=""
        )

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        plans, "resolve_executable", lambda value, **kwargs: Path(value)
    )
    assert (
        plans._pin_rustup_proxy(selector, role="rustc", root=tmp_path, env={})
        == compiler
    )
    assert calls == [[str(proxy), "which", "rustc"]]
    selector.write_bytes(b"MZcustom")
    assert (
        plans._pin_rustup_proxy(selector, role="rustc", root=tmp_path, env={})
        == selector
    )
    assert len(calls) == 1


@pytest.mark.parametrize("mutation", ["rustc", "resource", "config"])
def test_supplied_manifest_cannot_attest_a_different_live_plan(
    plan_root: Path, mutation: str
) -> None:
    plan = _plan(plan_root)
    content = {
        "tools": {
            item.label.split("/", 1)[1]: {
                "logical_name": item.label.split("/", 1)[1],
                **item.content_record(),
            }
            for item in plan.executable_custody
        },
        "wrappers": {},
        "effective_target": plan.target,
        "cargo_configuration": plan.configuration_identity(),
        "rust_resources": {
            "host_triple": plan.host_target,
            "selected_target": plan.target,
            "content": plan.rust_resources.content_identity(),
        },
    }
    _verify_plan_toolchain_content(plan, content)
    if mutation == "rustc":
        plan.rustc.write_bytes(b"MZnew compiler")
    elif mutation == "resource":
        (plan_root / "rust-resources" / "libcore.rlib").write_bytes(b"new core")
    else:
        _config(plan_root, '[build]\nrustflags="--cfg changed"\n')
    replacement = _plan(plan_root)
    with pytest.raises(ValueError, match="differs|differ"):
        _verify_plan_toolchain_content(replacement, content)


@pytest.mark.parametrize("selector", ["extern", "search", "codegen", "linker"])
def test_flag_selected_resources_are_captured_and_fenced(
    plan_root: Path, selector: str
) -> None:
    directory = plan_root / "custom resources"
    directory.mkdir()
    artifact = directory / "resource.bin"
    artifact.write_bytes(b"MZresource")
    flags = {
        "extern": ("--extern", "dep=" + str(artifact)),
        "search": ("-L", "native=" + str(directory)),
        "codegen": ("-Z", "codegen-backend=" + str(artifact)),
        "linker": ("-C", "linker=selected-linker"),
    }[selector]
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    selected = plan.tools["linker"] if selector == "linker" else artifact
    custody = (
        plan.executable_custody if selector == "linker" else plan.rust_resources.files
    )
    assert any(item.identity.path == selected for item in custody)
    selected.write_bytes(b"changed resource")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_final_crate_resource_override_retains_dependency_custody(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = plan_root / "dependency-sysroot"
    first.mkdir()
    seen = []

    def roots(*args, **kwargs):
        seen.append(kwargs["flag_lanes"])
        return ()

    monkeypatch.setattr(plans, "_rust_resource_roots", roots)
    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(
                ("--sysroot", str(first), "-C", "linker=dependency-linker")
            )
        },
        args=("--", "-C", "linker=final-linker"),
    )
    assert seen == [
        (
            plan.rustflags,
            (*plan.rustflags, "-C", "linker=" + str(plan.tools["final_linker"])),
        )
    ]
    assert plan.tools["linker"].name == "dependency-linker"
    assert plan.tools["final_linker"].name == "final-linker"
    custody = {item.label: item.entrypoint for item in plan.executable_custody}
    assert custody["tool/linker"] == plan.tools["linker"]
    assert custody["tool/final_linker"] == plan.tools["final_linker"]
    assert not plan.rust_resources.files
    assert not any(label.endswith("/linker") for label, _ in plan.resource_paths)
    assert first in dict(plan.resource_paths).values()


def test_duplicate_sysroot_across_dependency_and_final_crate_is_diagnosed(
    plan_root: Path,
) -> None:
    sysroot = plan_root / "sysroot"
    sysroot.mkdir()
    with pytest.raises(ValueError, match="duplicate --sysroot"):
        _plan(
            plan_root,
            env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(("--sysroot", str(sysroot)))},
            args=("--", "--sysroot", str(sysroot)),
        )


@pytest.mark.parametrize("role", ["linker", "final_linker"])
def test_rust_linker_executable_mutation_is_fenced(plan_root: Path, role: str) -> None:
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-C linker=dependency-linker"},
        args=("--", "-C", "linker=final-linker"),
    )
    selected = plan.tools[role]
    before = selected.stat()
    selected.write_bytes(b"MZ" + b"x" * (before.st_size - 2))
    os.utime(selected, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["command", "flags", "missing-tool"])
def test_rust_linker_selector_must_reconcile_with_executable_custody(
    plan_root: Path, mutation: str
) -> None:
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-C linker=dependency-linker"},
        args=("--", "-C", "linker=final-linker"),
    )
    if mutation == "command":
        plan = replace(
            plan,
            command=(*plan.command, "-C", "linker=" + str(plan.tools["linker"])),
        )
    elif mutation == "flags":
        flags = ("-C", "linker=" + str(plan.tools["final_linker"]))
        plan = replace(
            plan,
            rustflags=flags,
            environment=MappingProxyType(
                {**plan.environment, "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)}
            ),
        )
    else:
        plan = replace(
            plan,
            tools=MappingProxyType(
                {
                    role: path
                    for role, path in plan.tools.items()
                    if role != "final_linker"
                }
            ),
        )
    with pytest.raises(ValueError, match="linker selection differs"):
        plan.verify()


def test_final_crate_inherits_dependency_linker_without_duplicate_role(
    plan_root: Path,
) -> None:
    plan = _plan(plan_root, env={"RUSTFLAGS": "-C linker=dependency-linker"})
    assert plan.tools["linker"].name == "dependency-linker"
    assert "final_linker" not in plan.tools
    assert not any(
        item.label == "tool/final_linker" for item in plan.executable_custody
    )
    assert not any(label.endswith("/linker") for label, _ in plan.resource_paths)


def test_final_crate_linker_last_selector_wins(plan_root: Path) -> None:
    plan = _plan(
        plan_root,
        args=("--", "-Clinker=missing/linker", "-C", "linker=final-linker"),
    )
    assert plan.tools["final_linker"].name == "final-linker"
    assert "-Clinker=missing/linker" not in plan.command


@pytest.mark.parametrize("final", [False, True])
@pytest.mark.parametrize("content", [b"#!/bin/sh\nexec ld\n", b"@echo off\nld.exe\n"])
def test_rust_linker_scripts_cannot_enter_executable_custody(
    plan_root: Path, final: bool, content: bytes
) -> None:
    linker = plan_root / "script-linker"
    linker.write_bytes(content)
    selector = ("-C", "linker=" + str(linker))
    with pytest.raises(ValueError, match="native executable, not a script"):
        _plan(
            plan_root,
            env={} if final else {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(selector)},
            args=("--", *selector) if final else (),
        )


def test_final_linker_is_an_exact_portable_toolchain_role() -> None:
    value = runtime_build_identity("shared").to_dict()
    tools = value["payload"]["family"]["compile"]["toolchain"]["tools"]
    tools["final_linker"] = {**tools["rustc"], "logical_name": "final_linker"}
    _reseal(value)
    assert RuntimeBuildIdentity.from_dict(value).to_dict() == value


def test_last_codegen_selector_wins_without_opening_shadowed_path(
    plan_root: Path,
) -> None:
    selected = plan_root / "backend.dll"
    selected.write_bytes(b"MZbackend")
    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(
                (
                    "-Zcodegen-backend=missing/backend.dll",
                    "-Z",
                    "codegen-backend=" + str(selected),
                )
            )
        },
    )
    assert plan.rustflags == ("-Z", "codegen-backend=" + str(selected))
    assert any(item.identity.path == selected for item in plan.rust_resources.files)


@pytest.mark.parametrize(
    "flags,diagnostic",
    [
        (("--sysroot", "a", "--sysroot", "b"), "duplicate --sysroot"),
        (("--extern", "dep"), "explicit crate=artifact"),
        (("-L", "unknown=somewhere"), "search kind"),
        (("-Z", "codegen-backend="), "requires a resource selector"),
        (("@arguments.rsp",), "parsed argument custody"),
    ],
)
def test_unresolved_rust_resource_selector_is_diagnosed(
    plan_root: Path, flags: tuple[str, ...], diagnostic: str
) -> None:
    with pytest.raises(ValueError, match=diagnostic):
        _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})


@pytest.mark.parametrize("wrapped", [False, True])
def test_selected_sysroot_supplies_target_resources_not_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, wrapped: bool
) -> None:
    compiler = tmp_path / "rustc"
    compiler.write_bytes(b"MZcompiler")
    installed, selected = tmp_path / "installed", tmp_path / "selected"
    (installed / "lib" / "rustlib" / "host" / "lib").mkdir(parents=True)
    selected_lib = selected / "lib" / "rustlib" / "target" / "lib"
    selected_lib.mkdir(parents=True)
    wrappers = {}
    if wrapped:
        for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            wrapper = tmp_path / role
            wrapper.write_bytes(b"MZwrapper")
            wrappers[role] = wrapper
    expected_prefix = [*(str(path) for path in wrappers.values()), str(compiler)]

    def probe(command, **kwargs):
        is_wrapped = wrapped and command[0] == expected_prefix[0]
        prefix = expected_prefix if is_wrapped else [str(compiler)]
        assert command[: len(prefix)] == prefix
        args = command[len(prefix) :]
        assert args[:4] == ["-", "--crate-name", "___", "--print=file-names"]
        assert kwargs["input"] == ""
        # The wrapper redirects crate queries even without an explicit
        # --sysroot. A shortened resource query would incorrectly pick installed.
        value = (
            selected
            if "--target" in args and (is_wrapped or "--sysroot" in args)
            else installed
        )
        return subprocess.CompletedProcess(
            command, 0, stdout=_metadata_stdout(value), stderr=""
        )

    monkeypatch.setattr(plans.process_guard, "run_completed_command", probe)
    roots = plans._rust_resource_roots(
        compiler,
        root=tmp_path,
        env={},
        target="target",
        host="host",
        flag_lanes=((),) if wrapped else (("--sysroot", str(selected)),),
        wrappers=wrappers,
        target_argument="target",
    )
    target_roots = [
        entry.path for entry in roots if entry.label.startswith("rust/target-libdir/")
    ]
    assert target_roots == [selected_lib]
    assert (
        next(entry.path for entry in roots if entry.label == "rust/driver")
        == installed / "lib"
    )


@pytest.mark.parametrize("mutated", [None, "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"])
def test_cfg_probe_uses_and_fences_nested_cargo_wrappers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutated: str | None
) -> None:
    rustc = tmp_path / "rustc"
    rustc.write_bytes(b"MZcompiler")
    wrappers = {}
    for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        path = tmp_path / role
        path.write_bytes(b"MZwrapper")
        wrappers[role] = path

    def run(command, **kwargs):
        assert command == [
            *(str(path) for path in wrappers.values()),
            str(rustc),
            *plans.cargo_target_query_arguments("target", ("--cfg", "selected")),
        ]
        if mutated is not None:
            wrappers[mutated].write_bytes(b"MZchanged")
        return subprocess.CompletedProcess(command, 0, _metadata_stdout(tmp_path), "")

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    if mutated is None:
        assert ("selected", None) in plans._rust_target_metadata(
            rustc,
            "target",
            ("--cfg", "selected"),
            root=tmp_path,
            env={},
            wrappers=wrappers,
        ).cfg
    else:
        with pytest.raises((ValueError, OSError), match="changed"):
            plans._rust_target_metadata(
                rustc,
                "target",
                ("--cfg", "selected"),
                root=tmp_path,
                env={},
                wrappers=wrappers,
            )

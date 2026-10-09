from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path
from types import MappingProxyType

import pytest

from molt.cli import runtime_cargo_plan as plans
from molt.cli import runtime_fingerprints as fingerprints
from molt.cli.runtime_build_identity import (
    _capture_plan_toolchain,
    _runtime_build_environment_identity,
    _verify_plan_toolchain_content,
)
from molt.cli.runtime_cargo_plan import _rust_resource_roots
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    runtime_build_fingerprint,
)
from molt.exact_json import canonical_json_sha256
from molt import rust_toolchain
from tests.executable_test_support import write_mock_executable
from tests.rustc_test_support import (
    rustc_target_metadata_output,
    rustc_target_metadata_stdout as _metadata_stdout,
)
from tests.runtime_build_identity_helper import (
    build_python_identity_fixture,
    RuntimeFixtureRoot,
    provisioned_wasi_sdk_fixture,
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
            write_mock_executable(path, b"MZ" + str(value).encode())
        return path

    monkeypatch.setattr(plans, "resolve_executable", executable)
    monkeypatch.setattr(
        plans.shutil, "which", lambda value, **kwargs: str(tmp_path / value)
    )

    def metadata_command(command, **kwargs):
        if tuple(command[1:]) == ("-vV",):
            # The real resolver queries its admitted physical Rust compiler
            # before selecting target metadata. Match this fixture's host.
            assert Path(command[0]) == Path(kwargs["env"]["RUSTC"])
            assert Path(command[0]).is_file()
            return subprocess.CompletedProcess(
                command, 0, "rustc fixture\nhost: x86_64-unknown-linux-gnu\n", ""
            )
        assert "--print=file-names" in command
        assert kwargs["input"] == ""
        target = (
            command[command.index("--target") + 1] if "--target" in command else None
        )
        stdout, stderr = rustc_target_metadata_output(tmp_path, target=target)
        return subprocess.CompletedProcess(command, 0, stdout, stderr)

    monkeypatch.setattr(plans.process_guard, "run_completed_command", metadata_command)
    resources = tmp_path / "rust-resources"
    resources.mkdir()
    (resources / "libcore.rlib").write_bytes(b"rust-core")
    monkeypatch.setattr(
        plans,
        "_rust_resource_roots",
        lambda *args, **kwargs: (plans.CargoResourceRoot("rust/test", resources),),
    )
    return tmp_path


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
    environment = {"CARGO_HOME": str(root / "cargo-home"), **(env or {})}
    if target in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(root))
        environment["WASI_SDK_PATH"] = str(installation.sdk)
    return plans.resolve_runtime_cargo_plan(
        root,
        env=environment,
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


@pytest.mark.parametrize("target", [None, "wasm32-wasip1"])
@pytest.mark.parametrize("locked", [(), ("--locked",)])
def test_runtime_plan_locks_compiler_dependencies_before_rustc_passthrough(
    plan_root: Path, target: str | None, locked: tuple[str, ...]
) -> None:
    plan = _plan(
        plan_root,
        args=(
            *(("--target", target) if target is not None else ()),
            *locked,
            "--",
            "--print",
            "native-static-libs",
        ),
        target=target,
        # _plan supplies a complete synthetic SDK for WASM. Native C tools
        # remain separately authored host inputs.
        env={"MOLT_SKIP_CARGO_LOCK": "1"}
        | {name: "selected-" + name.lower() for name in ("CC", "CXX", "AR", "RANLIB")},
    )
    separator = plan.command.index("--")
    assert plan.command[:separator].count("--locked") == 1
    assert plan.command[separator : separator + 3] == (
        "--",
        "--print",
        "native-static-libs",
    )
    assert plan.command[separator + 3 :] == (
        ("-C", "link-self-contained=no", "-C", "linker-flavor=wasm-ld")
        if target == "wasm32-wasip1"
        else ()
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
    expected = ("--cfg", "wasm_selected" if wasm else "baseline")
    if target == "wasm32-wasip1":
        expected = (
            "-L",
            "native=" + str(plan.wasi_c_abi.path("libc").parent),
            "-L",
            "native=" + str(plan.wasi_c_abi.path("compiler_rt").parent),
            "-C",
            "link-self-contained=no",
            "-C",
            "linker-flavor=wasm-ld",
            *expected,
        )
    assert plan.rustflags == expected
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert seen and all(selected == target for selected, _ in seen)
    if target == "wasm32-wasip1":
        assert plan.tools["linker"] == Path(plan.environment["MOLT_WASM_LD"])
        assert all("wasm-linker" not in token for token in plan.command)
    elif wasm:
        assert plan.tools["linker"].name == "wasm-linker"
        assert any("wasm-linker" in token for token in plan.command)


@pytest.mark.parametrize("failure", ["exit", "empty", "mutated", "none"])
def test_cfg_probe_retains_selected_compiler_and_reports_failure(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    rustc = plan_root / "cfg-rustc"
    write_mock_executable(rustc, b"compiler-before")
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
    before_identity = plan.configuration_identity()
    assert plan.configuration_identity() == before_identity
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["rustc", "resource", "config", "new-config"])
def test_native_capture_closes_custody_after_python_probe(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    from molt.cli import runtime_build_identity as identity
    from tests.runtime_build_identity_helper import build_python_identity_fixture

    config = _config(plan_root, '[build]\nrustflags="--cfg before"\n')
    plan = _plan(plan_root)

    def capture_python(_env, **_kwargs):
        if mutation == "new-config":
            extra = plan_root / "cargo-home" / "config.toml"
            extra.parent.mkdir(exist_ok=True)
            extra.write_text('[build]\nrustflags="--cfg inserted"\n', encoding="utf-8")
        else:
            path = {
                "rustc": plan.tools["rustc"],
                "resource": plan_root / "rust-resources" / "libcore.rlib",
                "config": config,
            }[mutation]
            before = path.stat()
            path.write_bytes(path.read_bytes().replace(b"before", b"after!") + b"!")
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return build_python_identity_fixture()

    monkeypatch.setattr(identity, "_python_identity", capture_python)
    with pytest.raises((ValueError, OSError), match="changed"):
        identity.provision_native_runtime_toolchain_content_manifest(
            project_root=plan_root,
            env=plan.environment,
            target_triple=None,
            cargo_command=plan.command,
            cargo_plan=plan,
        )


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
        "capture_stable_regular_file",
        lambda *args, **kwargs: pytest.fail(
            "response bytes were independently recaptured"
        ),
    )
    projected = plan.project_link_arguments(plan.partition_command()[1])
    assert any("@response:sha256=" in item for item in projected)


def test_link_response_aliases_share_one_bounded_byte_capture(tmp_path, monkeypatch):
    response = tmp_path / "exports.rsp"
    response.write_bytes(b"--export=first\n")
    original = plans.capture_stable_regular_file
    captures = []

    def capture(path, **kwargs):
        assert kwargs["max_bytes"] == plans.RUNTIME_ARTIFACT_METADATA_MAX_BYTES
        captures.append(path)
        return original(path, **kwargs)

    monkeypatch.setattr(plans, "capture_stable_regular_file", capture)
    roots = (
        plans.CargoResourceRoot("ordinary-first", response),
        plans.CargoResourceRoot("rust/link-response/first", response),
        plans.CargoResourceRoot("rust/link-response/second", response),
    )
    custody = plans.CargoResourceCustody.capture(roots)
    assert captures == [response]
    assert len(custody.files) == 3
    assert all(item.identity is custody.files[0].identity for item in custody.files)


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


def test_plan_owns_final_selected_inputs(plan_root: Path) -> None:
    _config(plan_root, '[env]\nCC={value="configured-cc",force=true}\n')
    plan = _plan(plan_root)
    assert plan.tools["cc"].name == "configured-cc"
    with pytest.raises(TypeError):
        plan.environment["CC"] = "other"
    assert any(item.entrypoint == plan.tools["cc"] for item in plan.executable_custody)
    plan.verify()


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


@pytest.mark.parametrize(
    ("profile", "ancestry"),
    [
        ("release-output", ("release-output", "release")),
        ("release-size", ("release-size", "release-output", "release")),
        ("wasm-release", ("wasm-release", "release-size", "release-output", "release")),
    ],
)
def test_workspace_shipping_profile_ancestry_is_consumed_by_runtime_identity(
    plan_root: Path, profile: str, ancestry: tuple[str, ...]
) -> None:
    workspace_manifest = Path(__file__).resolve().parents[2] / "Cargo.toml"
    (plan_root / "Cargo.toml").write_bytes(workspace_manifest.read_bytes())
    baseline = _plan(plan_root)
    inherited = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2",
            "CARGO_PROFILE_RELEASE_SIZE_LTO": "off",
        },
    )
    assert inherited.profile_ancestry(profile) == ancestry
    expected = {"CARGO_PROFILE_RELEASE_OUTPUT_DEBUG"}
    if "release-size" in ancestry:
        expected.add("CARGO_PROFILE_RELEASE_SIZE_LTO")
    assert set(inherited.profile_environment(profile)) == expected
    assert inherited.preserve_debug_for_profile(profile)
    assert not baseline.preserve_debug_for_profile(profile)
    assert _runtime_build_environment_identity(inherited, cargo_profile=profile) != (
        _runtime_build_environment_identity(baseline, cargo_profile=profile)
    )


@pytest.mark.parametrize("source", ["manifest", "config", "cli-file", "cli-inline"])
def test_unselected_profile_namespace_does_not_pollute_runtime_identity(
    plan_root: Path, source: str
) -> None:
    manifest = plan_root / "Cargo.toml"
    selected = '[profile.release-output]\ninherits="release"\n'
    sibling = '[profile.release-size]\ninherits="release-output"\n'
    manifest.write_text(
        selected + (sibling if source == "manifest" else ""), encoding="utf-8"
    )
    args: tuple[str, ...] = ()
    if source == "config":
        config = plan_root / ".cargo" / "config.toml"
        config.parent.mkdir(exist_ok=True)
        config.write_text(sibling, encoding="utf-8")
    elif source == "cli-file":
        config = plan_root / "profiles.toml"
        config.write_text(sibling, encoding="utf-8")
        args = ("--config", str(config))
    elif source == "cli-inline":
        args = ("--config", 'profile.release-size.inherits="release-output"')
    selected_env = {
        "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL": "3",
    }
    baseline = _plan(plan_root, args=args, env=selected_env)
    sibling_env = {
        "CARGO_PROFILE_RELEASE_SIZE_LTO": "off",
        "CARGO_PROFILE_RELEASE_SIZE_BUILD_OVERRIDE_DEBUG": "0",
        "CARGO_PROFILE_RELEASE_SIZE_FUTURE_CONTROL": "1",
        "CARGO_PROFILE_BENCH_DEBUG": "1",
    }
    with_sibling = _plan(plan_root, args=args, env=selected_env | sibling_env)
    assert with_sibling.profile_environment("release-output") == selected_env
    assert _runtime_build_environment_identity(
        baseline, cargo_profile="release-output"
    ) == _runtime_build_environment_identity(
        with_sibling, cargo_profile="release-output"
    )
    with pytest.raises(ValueError, match="RELEASE_SIZE_FUTURE_CONTROL"):
        with_sibling.profile_environment("release-size")


@pytest.mark.parametrize(
    "profile", ["release-build-override", "release_build_override"]
)
def test_legal_ancestor_controls_survive_profile_namespace_overlap(
    plan_root: Path, profile: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        f'[profile.{profile}]\ninherits="dev"\n'
        '[profile.release-debug]\ninherits="dev"\n',
        encoding="utf-8",
    )
    shared = "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL"
    assertions = "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"
    environment = {shared: "3", assertions: "true"}
    plan = _plan(plan_root, env=environment)
    # These are legal release controls even when unrelated longer profile names
    # exist. The shared key is also the custom profile's own opt-level control.
    assert plan.profile_environment("release") == environment
    assert plan.profile_environment(profile) == {shared: "3"}
    assert plan.profile_environment("dev") == {}
    assert plan.profile_environment("release-debug") == {}
    release_identity = _runtime_build_environment_identity(
        plan, cargo_profile="release"
    )
    custom_identity = _runtime_build_environment_identity(plan, cargo_profile=profile)
    assert release_identity[shared] == custom_identity[shared]
    changed = _plan(plan_root, env=environment | {shared: "2"})
    for selected in ("release", profile):
        assert (
            _runtime_build_environment_identity(changed, cargo_profile=selected)[shared]
            != release_identity[shared]
        )


@pytest.mark.parametrize("profile", ["release-output", "release_output"])
def test_profile_environment_aliases_share_cargo_key(
    plan_root: Path, profile: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n'
        '[profile.release_output]\ninherits="dev"\n',
        encoding="utf-8",
    )
    environment = {"CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2"}
    plan = _plan(plan_root, env=environment)
    assert plan.profile_environment(profile) == environment
    assert plan.preserve_debug_for_profile(profile)


@pytest.mark.parametrize(
    "control",
    [
        "CARGO_PROFILE_CUSTOM_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_BUILD_OVERRIDE_DEBUG",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_PANIC",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_LTO",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_RPATH",
    ],
)
def test_unknown_inherited_profile_control_is_diagnosed(
    plan_root: Path, control: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.custom]\ninherits="release"\n', encoding="utf-8"
    )
    with pytest.raises(ValueError, match="unsupported output-bearing"):
        _plan(plan_root, env={control: "1"}).profile_environment("custom")


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
    from molt import process_guard

    suffix = ".exe" if os.name == "nt" else ""
    proxy = tmp_path / ("rustup" + suffix)
    selector = tmp_path / ("rustc" + suffix)
    compiler = tmp_path / ("selected-rustc" + suffix)
    write_mock_executable(proxy, b"MZrustup")
    write_mock_executable(selector, proxy.read_bytes())
    write_mock_executable(compiler, b"MZcompiler")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(
            command, 0, stdout=str(compiler) + "\n", stderr=""
        )

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        rust_toolchain, "resolve_executable", lambda value, **kwargs: Path(value)
    )
    assert (
        rust_toolchain.resolve_rustup_proxy(
            selector, role="rustc", root=tmp_path, env={}
        )
        == compiler
    )
    assert calls == [[str(proxy), "which", "rustc"]]
    selector.write_bytes(b"MZcustom")
    assert (
        rust_toolchain.resolve_rustup_proxy(
            selector, role="rustc", root=tmp_path, env={}
        )
        == selector
    )
    assert len(calls) == 1


@pytest.mark.parametrize("mutation", ["rustc", "resource", "config"])
def test_supplied_manifest_cannot_attest_a_different_live_plan(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
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
        "sysroots": {},
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
    monkeypatch.setattr(
        "molt.cli.runtime_build_identity._python_identity",
        lambda *_args, **_kwargs: build_python_identity_fixture(),
    )
    # Captured projections remain pure. Runtime admission owns the live
    # mutable-input fence before those facts can authorize execution.
    with pytest.raises(ValueError, match="changed"):
        _capture_plan_toolchain(plan)
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
        # Replace the admitted selector in place. Appending a duplicate is
        # already rejected by canonical flag admission and never reaches the
        # independent executable-custody reconciliation tested here.
        original = "linker=" + str(plan.tools["final_linker"])
        assert plan.command.count(original) == 1
        plan = replace(
            plan,
            command=tuple(
                "linker=" + str(plan.tools["linker"]) if token == original else token
                for token in plan.command
            ),
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
    write_mock_executable(linker, content)
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
        (("-L", "unknown=somewhere"), "resource is missing"),
        (("-Z", "codegen-backend="), "requires a resource selector"),
        (("@arguments.rsp",), "parsed argument custody"),
        (("-o", "@arguments.rsp"), "parsed argument custody"),
        (("--", "@arguments.rsp"), "parsed argument custody"),
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
    write_mock_executable(compiler, b"MZcompiler")
    installed, selected = tmp_path / "installed", tmp_path / "selected"
    (installed / "lib" / "rustlib" / "host" / "lib").mkdir(parents=True)
    selected_lib = selected / "lib" / "rustlib" / "target" / "lib"
    selected_lib.mkdir(parents=True)
    wrappers = {}
    if wrapped:
        for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            wrapper = tmp_path / role
            write_mock_executable(wrapper, b"MZwrapper")
            wrappers[role] = wrapper
    expected_prefix = [*wrappers.values(), compiler]

    def probe(command, **kwargs):
        is_wrapped = wrapped and Path(command[0]) == expected_prefix[0]
        prefix = expected_prefix if is_wrapped else [compiler]
        assert [Path(arg) for arg in command[: len(prefix)]] == prefix
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
    write_mock_executable(rustc, b"MZcompiler")
    wrappers = {}
    for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        path = tmp_path / role
        write_mock_executable(path, b"MZwrapper")
        wrappers[role] = path

    def run(command, **kwargs):
        prefix = [*wrappers.values(), rustc]
        assert [Path(arg) for arg in command[: len(prefix)]] == prefix
        assert tuple(command[len(prefix) :]) == plans.cargo_target_query_arguments(
            "target", ("--cfg", "selected")
        )
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


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "toolchain"])
def test_validated_runtime_receipts_reuse_only_exact_immutable_instances(
    kind: str,
) -> None:
    receipt = (
        native_runtime_staticlib_identity()
        if kind == "native"
        else runtime_build_identity("shared" if kind == "toolchain" else kind)
    )
    if kind == "toolchain":
        receipt = receipt.toolchain_manifest
    cls = type(receipt)
    assert cls.from_dict(receipt) is receipt
    detached = receipt.to_dict()
    admitted = cls.from_dict(detached)
    detached["payload"].clear()
    assert admitted == receipt
    with pytest.raises(ValueError):
        cls.from_dict(detached)
    assert dict(receipt) == receipt.to_dict()
    projected = receipt["payload"]
    projected.clear()
    assert receipt == admitted
    with pytest.raises(KeyError):
        receipt["not-a-field"]


@pytest.mark.parametrize("kind", ["build", "toolchain"])
def test_runtime_receipt_subclasses_cannot_bypass_admission(kind: str) -> None:
    receipt = runtime_build_identity("shared")
    if kind == "toolchain":
        receipt = receipt.toolchain_manifest
    cls = type(receipt)

    class UnvalidatedReceipt(cls):
        def __post_init__(self) -> None:
            pass

    values = {key: getattr(receipt, key) for key in ("digest", "payload")}
    if kind == "build":
        values.update(
            compile_digest=receipt.compile_digest, family_digest=receipt.family_digest
        )
    values["digest"] = "0" * 64
    unvalidated = UnvalidatedReceipt(**values)
    with pytest.raises(ValueError, match="digest"):
        cls.from_dict(unvalidated)


@pytest.mark.parametrize("kind", ["shared", "reloc", "staticlib"])
def test_expected_runtime_identity_admits_only_its_canonical_content(kind) -> None:
    expected = (
        native_runtime_staticlib_identity()
        if kind == "staticlib"
        else runtime_build_identity(kind)
    )
    wire = expected.to_dict()
    assert RuntimeBuildIdentity.from_dict(wire, expected=expected) is expected
    assert RuntimeBuildIdentity.from_dict(expected, expected=expected) is expected
    # Admission retains the trusted owner, never the caller's wire backing.
    wire["payload"].clear()
    with pytest.raises(ValueError, match="shape is invalid"):
        RuntimeBuildIdentity.from_dict(wire, expected=expected)
    assert RuntimeBuildIdentity.from_dict(expected.to_dict()) == expected


@pytest.mark.parametrize(
    "mutation",
    ["digest", "compile_digest", "family_digest", "extra", "resealed", "bool"],
)
def test_expected_runtime_identity_rejects_changed_or_resealed_claims(mutation) -> None:
    expected = runtime_build_identity("shared")
    wire = expected.to_dict()
    if mutation in {"digest", "compile_digest", "family_digest"}:
        wire[mutation] = "0" * 64
    elif mutation == "extra":
        wire["extra"] = None
    else:
        member = wire["payload"]["family"]["members"]["shared"]
        if mutation == "bool":
            # Python equality conflates True with 1; canonical JSON does not.
            member["preserve_debug"] = int(member["preserve_debug"])
        else:
            member["publication_transform"] = "different-publication"
            _reseal(wire)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(wire, expected=expected)


def test_expected_runtime_identity_reuses_its_admitted_facts(monkeypatch) -> None:
    from molt.cli import runtime_identity_schema as schema

    expected = runtime_build_identity("shared")
    wire = expected.to_dict()

    def no_second_semantic_admission(*_args, **_kwargs):
        raise AssertionError("trusted identity was semantically re-admitted")

    monkeypatch.setattr(
        schema, "_validated_runtime_build_payload", no_second_semantic_admission
    )
    assert RuntimeBuildIdentity.from_dict(wire, expected=expected) is expected


def test_owned_frozen_json_derivations_preserve_wire_and_isolation(monkeypatch) -> None:
    from molt.cli import runtime_identity_schema as schema
    from molt.exact_json import canonical_json_bytes

    mutable = {"nested": {"values": [1, 1.0, -0.0, True, None, "\u2603"]}}
    expected_bytes = canonical_json_bytes(mutable)
    expected_digest = canonical_json_sha256(mutable)
    proxy = MappingProxyType(mutable)
    frozen = schema._freeze_json(proxy)
    assert schema._canonical_json(frozen).encode("utf-8") == expected_bytes
    encoded = []
    original = schema.canonical_json_sha256

    def counted(value, **kwargs):
        encoded.append(value)
        return original(value, **kwargs)

    monkeypatch.setattr(schema, "canonical_json_sha256", counted)
    assert schema._digest(frozen) == expected_digest
    assert schema._digest(frozen) == expected_digest
    assert len(encoded) == 1
    mutable["nested"]["values"][0] = 9
    assert schema._digest(frozen) == expected_digest
    assert schema._digest(proxy) == canonical_json_sha256(mutable)
    assert schema._digest(proxy) != expected_digest
    detached = schema._thaw_json(frozen)
    detached["nested"]["values"].clear()
    assert schema._canonical_json(frozen).encode("utf-8") == expected_bytes
    with pytest.raises(TypeError):
        frozen["nested"]["values"][0] = 8
    with pytest.raises(TypeError):
        frozen["nested"]["extra"] = 8


def test_frozen_json_subclass_cannot_claim_owned_immutability() -> None:
    from molt.cli import runtime_identity_schema as schema

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    source = {"nested": {"value": 1}}
    unowned = Unowned(source)
    frozen = schema._freeze_json(unowned)
    source["nested"]["value"] = 2
    assert schema._thaw_json(frozen) == {"nested": {"value": 1}}
    assert schema._digest(unowned) == canonical_json_sha256(source)


@pytest.mark.parametrize(
    "invalid", [{1: "key"}, {"x": float("nan")}, {"x": float("inf")}, {"x": object()}]
)
def test_owned_frozen_json_rejects_inexact_values(invalid) -> None:
    from molt.cli import runtime_identity_schema as schema

    with pytest.raises((TypeError, ValueError)):
        schema._freeze_json(invalid)


@pytest.mark.parametrize("kind", ["str", "int", "float", "key"])
def test_owned_json_rejects_scalar_subclasses_and_forged_digest(kind) -> None:
    from molt.cli import runtime_identity_schema as schema

    conversions = []

    class MutableStr(str):
        def __str__(self):
            conversions.append("str")
            return "changed" if self.changed else "value"

    class MutableInt(int):
        def __int__(self):
            conversions.append("int")
            return -1 if self.changed else 1

    class MutableFloat(float):
        def __float__(self):
            conversions.append("float")
            return float("nan") if self.changed else 1.0

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    scalar = {
        "str": lambda: MutableStr("value"),
        "int": lambda: MutableInt(1),
        "float": lambda: MutableFloat(1.0),
        "key": lambda: MutableStr("key"),
    }[kind]()
    scalar.changed = False
    source = {scalar: 1} if kind == "key" else {"value": scalar}
    foreign = Unowned(source)
    object.__setattr__(foreign, "_cached_digest", "0" * 64)
    object.__setattr__(foreign, "_validated_schemas", frozenset({("forged",)}))
    for changed in (False, True):
        scalar.changed = changed
        for value in (source, foreign):
            with pytest.raises(TypeError):
                schema._freeze_json(value)
            with pytest.raises(TypeError):
                schema._digest(value)
    assert conversions == []


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_runtime_family_shares_immutable_capture_and_exports_detached_wire(
    monkeypatch, target
) -> None:
    from molt.cli import runtime_build_identity as builder
    from molt.cli import runtime_identity_schema as schema

    original = (
        native_runtime_staticlib_identity()
        if target == "native"
        else runtime_build_identity("shared")
    )
    wire = original.to_dict()
    validations = []
    validate = schema.validate_python_runtime_identity

    def counted(value):
        validations.append(value)
        return validate(value)

    monkeypatch.setattr(schema, "validate_python_runtime_identity", counted)
    captured = RuntimeBuildIdentity.from_dict(wire)
    family = captured.payload["family"]
    compilation = family["compile"]
    manifest = captured.toolchain_manifest
    python = compilation["toolchain"]["tools"]["build_python"]
    assert schema._runtime_toolchain_build_python(manifest) is python
    assert schema._runtime_toolchain_build_python(manifest) is python
    members = tuple(
        schema.RuntimeBuildMemberPlan(
            kind=kind,
            resolved_rustflags=tuple(member["resolved_rustflags"]),
            link_args=tuple(member["link_args"]),
            publication_transform=member["publication_transform"],
            preserve_debug=member["preserve_debug"],
        )
        for kind, member in family["members"].items()
    )
    identities = builder._resolve_runtime_build_family_identities(
        sources=compilation["sources"],
        toolchain_manifest=manifest,
        target_triple=compilation["common_config"]["target_triple"],
        common_config=compilation["common_config"],
        publication_authority=family["publication_authority"],
        members=members,
    )
    assert len(identities) == (1 if target == "native" else 2)
    assert all(
        identity.payload["family"] is identities[0].payload["family"]
        for identity in identities
    )
    assert (
        identities[0].payload["family"]["compile"]["toolchain"]
        is manifest.payload["toolchain"]
    )
    for identity in identities:
        assert (
            identity.toolchain_manifest.payload["toolchain"] is compilation["toolchain"]
        )
        wire = identity.to_dict()
        assert canonical_json_sha256(wire["payload"]) == identity.digest
        assert (
            canonical_json_sha256(wire["payload"]["family"]) == identity.family_digest
        )
        assert (
            canonical_json_sha256(wire["payload"]["family"]["compile"])
            == identity.compile_digest
        )
        wire["payload"]["family"]["compile"]["toolchain"].clear()
        assert identity.payload["family"]["compile"]["toolchain"]
    # Every projection consumes one admitted graph; a new wire graph must still
    # receive its own complete Python closure admission.
    assert len(validations) == 1
    detached = original.to_dict()
    assert RuntimeBuildIdentity.from_dict(detached) == original
    assert len(validations) == 2


@pytest.mark.parametrize("origin", ["mutable", "proxy", "subclass"])
def test_schema_admission_does_not_trust_foreign_success_markers(origin) -> None:
    from molt.cli import runtime_identity_schema as schema
    from tests.runtime_build_identity_helper import build_python_identity_fixture

    source = build_python_identity_fixture()
    owned = schema._freeze_json(source)
    schema._validated_build_python_identity(owned)

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    value = source
    if origin == "proxy":
        value = MappingProxyType(source)
    elif origin == "subclass":
        value = Unowned(source)
        object.__setattr__(value, "_validated_schemas", owned._validated_schemas)
    assert schema._validated_build_python_identity(value) is value
    source["selected_executable"]["size"] = -1
    with pytest.raises(ValueError, match="executable identity is invalid"):
        schema._validated_build_python_identity(value)
    # Admission never lends the caller an alias into the captured graph.
    assert schema._validated_build_python_identity(owned) is owned
    assert owned["selected_executable"]["size"] == 6


def test_schema_admission_never_caches_failure_or_confuses_schemas(monkeypatch) -> None:
    from molt.cli import runtime_identity_schema as schema
    from tests.runtime_build_identity_helper import build_python_identity_fixture

    source = build_python_identity_fixture()
    source["identity_sha256"] = "0" * 64
    invalid = schema._freeze_json(source)
    validations = []
    validate = schema.validate_python_runtime_identity

    def counted(value):
        validations.append(value)
        return validate(value)

    monkeypatch.setattr(schema, "validate_python_runtime_identity", counted)
    for _ in range(2):
        with pytest.raises(ValueError, match="Python identity digest is invalid"):
            schema._validated_build_python_identity(invalid)
    assert len(validations) == 2
    valid = schema._freeze_json(build_python_identity_fixture())
    schema._validated_build_python_identity(valid)
    with pytest.raises(ValueError, match="manifest content shape is invalid"):
        schema._validated_toolchain_manifest_payload(valid)


def test_toolchain_admission_keeps_target_and_outer_digest_boundaries() -> None:
    from molt.cli import runtime_identity_schema as schema

    identity = native_runtime_staticlib_identity()
    manifest = identity.toolchain_manifest
    toolchain = manifest.payload["toolchain"]
    assert (
        schema._validated_runtime_toolchain_content(toolchain, target_triple="native")
        is toolchain
    )
    assert (
        schema._validated_runtime_toolchain_content(
            toolchain, target_triple=toolchain["effective_target"]
        )
        is toolchain
    )
    with pytest.raises(ValueError, match="Rust target identity is invalid"):
        schema._validated_runtime_toolchain_content(
            toolchain, target_triple="wasm32-wasip1"
        )
    # Valid descendants do not authorize a new claimed digest for their parent.
    with pytest.raises(ValueError, match="manifest digest is invalid"):
        schema.RuntimeToolchainContentManifest("0" * 64, manifest.payload)
    with pytest.raises(ValueError, match="build identity digest is invalid"):
        schema.RuntimeBuildIdentity(
            "0" * 64, identity.compile_digest, identity.family_digest, identity.payload
        )


def test_receipt_admission_rejects_scalar_subclass_claims_and_targets() -> None:
    from molt.cli import runtime_identity_schema as schema

    class MutableClaim(str):
        changed = False
        __hash__ = str.__hash__

        def __eq__(self, other):
            return self.changed or str.__eq__(self, other)

    identity = native_runtime_staticlib_identity()
    manifest = identity.toolchain_manifest
    for changed in (False, True):
        MutableClaim.changed = changed
        for field in ("digest", "compile_digest", "family_digest"):
            claim = MutableClaim(getattr(identity, field))
            with pytest.raises(ValueError, match="digest shape is invalid"):
                replace(identity, **{field: claim})
            wire = identity.to_dict()
            wire[field] = claim
            with pytest.raises(ValueError, match="identity is incomplete"):
                schema.RuntimeBuildIdentity.from_dict(wire, expected=identity)
        with pytest.raises(ValueError, match="manifest digest is invalid"):
            replace(manifest, digest=MutableClaim(manifest.digest))
        with pytest.raises(ValueError, match="toolchain target is invalid"):
            schema._validated_runtime_toolchain_content(
                manifest.payload["toolchain"], target_triple=MutableClaim("native")
            )
        for receipt in (identity, manifest):
            wire = receipt.to_dict()
            wire["schema"] = MutableClaim(wire["schema"])
            with pytest.raises(ValueError, match="schema is invalid"):
                type(receipt).from_dict(wire)


@pytest.mark.parametrize("origin", ["capture", "wire"])
def test_toolchain_manifest_owns_one_canonical_payload(monkeypatch, origin) -> None:
    from molt.cli import runtime_identity_schema as schema

    original = native_runtime_staticlib_identity().toolchain_manifest
    wire = original.to_dict()
    encoded = []
    encode = schema.canonical_json_sha256

    def counted(value, **kwargs):
        if isinstance(value, Mapping) and set(value) == {
            "target_triple",
            "toolchain",
        }:
            encoded.append(value)
        return encode(value, **kwargs)

    monkeypatch.setattr(schema, "canonical_json_sha256", counted)
    admitted = (
        schema.RuntimeToolchainContentManifest.from_payload(wire["payload"])
        if origin == "capture"
        else schema.RuntimeToolchainContentManifest.from_dict(wire)
    )
    assert admitted.to_dict() == wire
    assert len(encoded) == 1
    assert encoded[0] is admitted.payload
    wire["payload"]["toolchain"]["tools"].clear()
    assert admitted == original
    with pytest.raises(ValueError, match="toolchain tools are invalid"):
        schema.RuntimeToolchainContentManifest.from_payload(wire["payload"])


def test_pure_freestanding_rust_does_not_select_c_sdk(plan_root, monkeypatch):
    monkeypatch.setattr(
        plans,
        "apply_provisioned_wasm_toolchain",
        lambda *a, **k: pytest.fail("freestanding Rust selected a C SDK"),
    )
    plan = _plan(
        plan_root,
        target="wasm32-unknown-unknown",
        args=("--target", "wasm32-unknown-unknown"),
    )
    assert plan.wasi_c_abi is None
    assert "MOLT_WASI_C_ABI_PLAN" not in plan.environment


def test_sdk_members_use_receipt_without_mutable_resource_capture(
    plan_root, monkeypatch
):
    captured = []
    original = plans.CargoFileCustody.capture.__func__

    def capture(cls, label, path):
        captured.append(path)
        return original(cls, label, path)

    monkeypatch.setattr(plans.CargoFileCustody, "capture", classmethod(capture))
    plan = _plan(plan_root, target="wasm32-wasip1", args=("--target", "wasm32-wasip1"))
    assert plan.wasi_c_abi is not None
    for _role, path, size, digest in plan.wasi_c_abi.files:
        assert captured.count(path) == 0
        assert not any(item.entrypoint == path for item in plan.rust_resources.files)
        fact = plan.wasi_sdk.facts["members"][_role]
        assert (fact["size"], fact["sha256"]) == (size, digest)


@pytest.mark.parametrize(
    "override",
    [None, "foreign-linker", "driver-flavor", "unstable-flavor", "self-contained"],
)
def test_explicit_freestanding_c_provider_uses_selected_raw_linker(plan_root, override):
    from molt import llvm_toolchain

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(plan_root))
    c_abi = llvm_toolchain.wasi_c_abi_plan(installation)
    environment = {"MOLT_WASI_C_ABI_PLAN": c_abi.encode()}
    if override == "foreign-linker":
        environment["CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] = "foreign-linker"
    elif override == "driver-flavor":
        environment["RUSTFLAGS"] = "-Clinker-flavor=wasm-lld-cc"
    elif override == "unstable-flavor":
        environment["RUSTFLAGS"] = "-Clinker-flavor=wasm-lld"
    elif override == "self-contained":
        environment["RUSTFLAGS"] = "-Clink-self-contained=yes"

    def resolve():
        return _plan(
            plan_root,
            target="wasm32-unknown-unknown",
            args=("--target", "wasm32-unknown-unknown"),
            env=environment,
        )

    if override is not None:
        with pytest.raises(
            ValueError,
            match="selected SDK raw linker|differs from the selected WASI SDK|conflicts with linker-flavor|conflicts with link-self-contained",
        ):
            resolve()
    else:
        plan = resolve()
        assert plan.wasi_c_abi == c_abi
        assert plan.tools["linker"] == c_abi.linker
        assert plan.rustflags == (
            "-L",
            "native=" + str(c_abi.path("libc").parent),
            "-L",
            "native=" + str(c_abi.path("compiler_rt").parent),
            "-C",
            "link-self-contained=no",
            "-C",
            "linker-flavor=wasm-ld",
        )
        assert plan.environment["CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] == str(
            c_abi.linker
        )
        plan.verify()


@pytest.mark.parametrize("source", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"])
def test_selected_sdk_search_context_precedes_user_roots_without_recapture(
    plan_root, monkeypatch, source
):
    foreign = plan_root / "foreign-libraries"
    foreign.mkdir()
    (foreign / "libc.a").write_bytes(b"foreign libc must not shadow selected SDK")
    flags = ("-L", "native=" + str(foreign), "--cfg", "user_preserved")
    key = "CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS" if source == "target" else source
    value = (
        "\x1f".join(flags)
        if source == "CARGO_ENCODED_RUSTFLAGS"
        else __import__("shlex").join(flags)
    )
    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(plan_root))
    captured = []
    original = plans.CargoResourceRoot.files

    def files(resource):
        assert not resource.path.is_relative_to(installation.sdk), (
            "managed SDK recursively recaptured"
        )
        captured.append(resource.path)
        return original(resource)

    monkeypatch.setattr(plans.CargoResourceRoot, "files", files)
    plan = _plan(
        plan_root,
        target="wasm32-wasip1",
        args=("--target", "wasm32-wasip1"),
        env={key: value},
    )
    libc_dir = plan.wasi_c_abi.path("libc").parent
    builtins_dir = plan.wasi_c_abi.path("compiler_rt").parent
    assert plan.rustflags[:6] == (
        "-L",
        "native=" + str(libc_dir),
        "-L",
        "native=" + str(builtins_dir),
        "-L",
        "native=" + str(foreign),
    )
    assert "user_preserved" in plan.rustflags
    assert foreign in captured
    assert {
        path for label, path in plan.logical_paths if label.startswith("wasi/search/")
    } == {libc_dir, builtins_dir}
    plan.verify()
    (foreign / "libc.a").write_bytes(b"mutated user input")
    with pytest.raises(ValueError):
        plan.verify()


def test_sdk_search_context_cannot_be_removed_from_retained_plan(plan_root):
    from dataclasses import replace
    from types import MappingProxyType

    plan = _plan(plan_root, target="wasm32-wasip1", args=("--target", "wasm32-wasip1"))
    flags = plan.rustflags[4:]
    damaged = replace(
        plan,
        rustflags=flags,
        environment=MappingProxyType(
            {
                **plan.environment,
                "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags),
            }
        ),
    )
    with pytest.raises(ValueError, match="admitted target context"):
        damaged.verify()


@pytest.mark.parametrize("simd,freestanding", [(True, False), (False, True)])
@pytest.mark.parametrize(
    "flag_origin", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]
)
def test_combined_runtime_plan_applies_target_policy_after_cargo_features(
    plan_root, simd, freestanding, flag_origin
):
    from molt.cli import runtime_wasm_build_spec as specs

    _config(
        plan_root,
        '[target.wasm32-wasip1]\nrustflags=["-C","target-feature=-reference-types"]\n',
    )
    sdk = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(plan_root))
    environment = {
        "CARGO_HOME": str(plan_root / "cargo-home"),
        "WASI_SDK_PATH": str(sdk.sdk),
    }
    if flag_origin != "target":
        separator = "\x1f" if flag_origin == "CARGO_ENCODED_RUSTFLAGS" else " "
        environment[flag_origin] = separator.join(
            ("-C", "target-feature=-reference-types")
        )
    shared = specs._RuntimeWasmBuildSpec(
        requested_cargo_profile="dev-fast",
        cargo_profile="dev-fast",
        profile_dir="dev-fast",
        incremental_enabled=False,
        env=environment,
        artifact_selection=specs.RUNTIME_CDYLIB_ARTIFACTS,
        runtime_exports="",
        link_flags="",
        cargo_rustflags="",
        fingerprint_rustflags="",
        no_default_features=True,
        wasm_cargo_features=(),
        fingerprint_features=(),
        fingerprint_path=plan_root / "shared.fingerprint.json",
        target_root=plan_root / "target",
        stored_fingerprint=None,
        fingerprint=None,
        staticlib_fingerprint=None,
    )
    reloc = shared._replace(artifact_selection=specs.RUNTIME_STATICLIB_ARTIFACTS)
    shared, reloc = specs._resolve_runtime_wasm_cargo_specs(
        plan_root,
        shared,
        reloc,
        simd_enabled=simd,
        freestanding=freestanding,
    )
    assert shared.cargo_plan is reloc.cargo_plan
    plan = shared.cargo_plan
    assert plan is not None
    assert plan.host_target == "x86_64-unknown-linux-gnu"
    # Literal expectations independent of the producer and SIMD receipt reader.
    feature = (
        "target-feature=-reference-types,+simd128"
        if simd
        else "target-feature=-reference-types,-simd128"
    )
    assert plan.rustflags.count("target-feature=-reference-types") == 1
    assert feature in plan.rustflags
    assert ('getrandom_backend="unsupported"' in plan.rustflags) is freestanding
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert shared.cargo_rustflags == reloc.cargo_rustflags
    index = plan.command.index("--crate-type")
    assert plan.command[index : index + 2] == ("--crate-type", "staticlib,cdylib")
    assert shared.fingerprint_rustflags == shared.cargo_rustflags
    assert reloc.fingerprint_rustflags == reloc.cargo_rustflags
    plan.verify()
    # Capture binds the resolved environment before the actual runtime caller
    # constructs its command again. Re-enter that caller, not plan.command
    # (which already contains the resolver's pinned Cargo configuration).
    repeated_shared, repeated_reloc = specs._resolve_runtime_wasm_cargo_specs(
        plan_root,
        shared,
        reloc,
        simd_enabled=simd,
        freestanding=freestanding,
    )
    repeated = repeated_shared.cargo_plan
    assert repeated is not None
    assert repeated is repeated_reloc.cargo_plan
    assert repeated.rustflags == plan.rustflags
    assert repeated.command == plan.command
    assert repeated.environment == plan.environment
    assert repeated.configuration_identity() == plan.configuration_identity()
    assert repeated_shared.fingerprint_rustflags == shared.fingerprint_rustflags
    assert repeated_reloc.fingerprint_rustflags == reloc.fingerprint_rustflags
    repeated.verify()


@pytest.mark.parametrize(
    "flag_origin", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]
)
def test_standalone_cpython_abi_plan_applies_simd_after_cargo_features(
    plan_root, monkeypatch, flag_origin
):
    from contextlib import nullcontext
    from molt.cli import runtime_wasm_build_support as support

    _config(
        plan_root,
        '[target.wasm32-wasip1]\nrustflags=["-C","target-feature=-reference-types"]\n',
    )
    sdk = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(plan_root))
    environment = {
        "CARGO_HOME": str(plan_root / "cargo-home"),
        "WASI_SDK_PATH": str(sdk.sdk),
    }
    if flag_origin != "target":
        separator = "\x1f" if flag_origin == "CARGO_ENCODED_RUSTFLAGS" else " "
        environment[flag_origin] = separator.join(
            ("-C", "target-feature=-reference-types")
        )
    monkeypatch.setattr(support, "build_python_scope", lambda state: nullcontext())
    monkeypatch.setattr(support, "_cargo_build_env", lambda: dict(environment))
    monkeypatch.setattr(
        support, "_cargo_target_root", lambda root: plan_root / "target"
    )
    monkeypatch.setattr(
        support,
        "_runtime_fingerprint_path",
        lambda *args: plan_root / "cpython.fingerprint.json",
    )

    class Planned(Exception):
        pass

    def capture_plan(*args, **kwargs):
        # Reach the real producer's transform and actual Cargo-plan admission,
        # then stop before build-Python setup, runtime identity or Cargo work.
        raise Planned(plans.resolve_runtime_cargo_plan(*args, **kwargs))

    monkeypatch.setattr(support, "resolve_runtime_cargo_plan", capture_plan)
    with pytest.raises(Planned) as stopped:
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=plan_root,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1,
        )
    plan = stopped.value.args[0]
    assert plan.host_target == "x86_64-unknown-linux-gnu"
    assert plan.rustflags.count("target-feature=-reference-types") == 1
    assert "target-feature=-reference-types,+simd128" in plan.rustflags
    assert not any("getrandom_backend=" in flag for flag in plan.rustflags)
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert "molt-lang-cpython-abi" in plan.command
    index = plan.command.index("--crate-type")
    assert plan.command[index : index + 2] == ("--crate-type", "staticlib")
    plan.verify()
    monkeypatch.setattr(support, "_cargo_build_env", lambda: dict(plan.environment))
    with pytest.raises(Planned) as stopped_again:
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=plan_root,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1,
        )
    repeated = stopped_again.value.args[0]
    assert repeated.rustflags == plan.rustflags
    assert repeated.command == plan.command
    assert repeated.environment == plan.environment
    assert repeated.configuration_identity() == plan.configuration_identity()
    repeated.verify()


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
@pytest.mark.parametrize("selector", ["linker", "link-arg"])
@pytest.mark.parametrize("final", [False, True])
def test_codegen_alias_resources_have_one_custody_and_partition(
    plan_root, spelling, selector, final
):
    artifact = plan_root / "selected resource.bin"
    artifact.write_bytes(
        b"--export=first\n" if selector == "link-arg" else b"MZbackend"
    )
    operand = (
        "selected-linker"
        if selector == "linker"
        else "@" + str(artifact)
        if selector == "link-arg"
        else str(artifact)
    )
    value = selector + "=" + operand
    flags = (
        (spelling, value)
        if spelling in {"-C", "--codegen"}
        else (("-C" if spelling == "-Cjoined" else spelling) + value,)
    )
    plan = _plan(
        plan_root,
        args=("--", *flags) if final else (),
        env={} if final else {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)},
    )
    lane = plan.command[plan.command.index("--") + 1 :] if final else plan.rustflags
    selected = (
        plan.tools["final_linker" if final else "linker"]
        if selector == "linker"
        else artifact
    )
    assert lane == (
        "-C",
        selector + "=" + ("@" if selector == "link-arg" else "") + str(selected),
    )
    if final and selector == "link-arg":
        compile_command, linking = plan.partition_command()
        assert linking == lane and value not in compile_command
        assert "@response:sha256=" in plan.project_link_arguments(linking)[1]
    plan.verify()
    selected.write_bytes(b"changed admitted input")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
@pytest.mark.parametrize(
    "option", ["linker-flavor=wasm-lld-cc", "link-self-contained=yes"]
)
def test_wasi_codegen_alias_conflict_fails_at_selected_mode(
    plan_root, spelling, option
):
    flags = (
        (spelling, option)
        if spelling in {"-C", "--codegen"}
        else (("-C" if spelling == "-Cjoined" else spelling) + option,)
    )
    with pytest.raises(ValueError, match="WASI external-libc mode conflicts"):
        _plan(
            plan_root,
            target="wasm32-wasip1",
            args=("--target", "wasm32-wasip1"),
            env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)},
        )


@pytest.mark.parametrize(
    "operand", ["-Clinker=tools/root", "--codegen=linker=tools/root"]
)
def test_runtime_search_operand_cannot_select_a_codegen_tool(plan_root, operand):
    directory = plan_root / operand
    directory.mkdir(parents=True)
    artifact = directory / "libopaque.a"
    artifact.write_bytes(b"opaque search input")
    flags = ("-L", operand, "--out-dir", operand, "--remap-path-prefix", operand)
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    assert plan.rustflags == ("-L", "all=" + str(directory), *flags[2:])
    assert any(item.identity.path == artifact for item in plan.rust_resources.files)
    assert all(not str(path).endswith("tools/root") for path in plan.tools.values())
    artifact.write_bytes(b"changed opaque input")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("output", ["-C", "-Lnative=unselected", "--codegen"])
def test_runtime_resource_and_partition_passes_preserve_output_operand(
    plan_root, output
):
    plan = _plan(plan_root, args=("--", "-o", output, "--codegen=panic=abort"))
    compile_command, link_args = plan.partition_command()
    assert compile_command[-5:] == ("--", "-o", output, "-C", "panic=abort")
    assert link_args == ()
    plan.verify()


@pytest.mark.parametrize(
    "cluster,option",
    [
        ("-gC", "linker=selected-linker"),
        ("-vC", "link_arg=@exports.rsp"),
        ("-gZ", "codegen_backend=backend.dll"),
    ],
)
def test_clustered_resource_options_keep_prefix_and_fence_selected_input(
    plan_root, cluster, option
):
    (plan_root / "exports.rsp").write_bytes(b"--export=first\n")
    (plan_root / "backend.dll").write_bytes(b"MZbackend")
    plan = _plan(plan_root, args=("--", cluster + option))
    compile_command, link_args = plan.partition_command()
    assert cluster[:-1] in compile_command
    if "link_arg=" in option:
        assert link_args == ("-C", "link-arg=@" + str(plan_root / "exports.rsp"))
        selected = plan_root / "exports.rsp"
    elif "codegen_backend=" in option:
        assert compile_command[-2:] == (
            "-Z",
            "codegen-backend=" + str(plan_root / "backend.dll"),
        )
        selected = plan_root / "backend.dll"
    else:
        selected = plan.tools["final_linker"]
        assert compile_command[-2:] == ("-C", "linker=" + str(selected))
    plan.verify()
    selected.write_bytes(b"changed clustered resource")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_codegen_key_aliases_obey_last_selector_without_reading_shadowed_path(
    plan_root,
):
    selected = plan_root / "backend.dll"
    selected.write_bytes(b"MZbackend")
    flags = ("-gZcodegen_backend=missing.dll", "-Z", "codegen-backend=" + str(selected))
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    assert plan.rustflags == ("-g", "-Z", "codegen-backend=" + str(selected))
    assert any(item.identity.path == selected for item in plan.rust_resources.files)

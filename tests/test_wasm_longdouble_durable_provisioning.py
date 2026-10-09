"""One coherent SDK C-runtime input family, with fail-before-link custody."""

from __future__ import annotations

import subprocess
import hashlib
from pathlib import Path

import pytest

from tests.runtime_build_identity_helper import RuntimeFixtureRoot
from molt.cli import wasm_link_inputs
from molt.cli import runtime_wasm_build_support as rb
from molt.llvm_toolchain import LlvmToolchainConfigError


def test_missing_sdk_refuses_c_runtime_instead_of_using_an_isolated_archive(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    for key in (
        "MOLT_WASI_SYSROOT",
        "WASI_SYSROOT",
        "WASI_SDK_PATH",
        "WASI_SDK_PREFIX",
        "MOLT_WASI_C_ABI_PLAN",
    ):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path))
    assert wasm_link_inputs.resolve_wasi_sysroot() is None
    with pytest.raises(LlvmToolchainConfigError, match="pinned WASI SDK is missing"):
        wasm_link_inputs.resolve_long_double_link_policy()


@pytest.fixture
def captured_cargo(runtime_fixture_root: RuntimeFixtureRoot, tmp_path: Path):
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    return runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env={},
        cargo_command=("cargo",),
        requested_target="wasm32-wasip1",
    )


@pytest.fixture
def selected_c_abi(captured_cargo, monkeypatch):
    plan = captured_cargo.wasi_c_abi
    monkeypatch.setenv("WASI_SDK_PATH", str(plan.sdk))
    monkeypatch.setenv("MOLT_WASI_SYSROOT", str(plan.sysroot))
    monkeypatch.setenv("WASI_SYSROOT", str(plan.sysroot))
    return plan


@pytest.mark.parametrize("missing", ("long_double", "compiler_rt", "libc"))
def test_explicit_sdk_verification_rejects_missing_archive(
    captured_cargo, missing: str
) -> None:
    from molt.llvm_toolchain import load_wasi_sdk_installation
    from molt.cli.compiler_metadata import _compiler_root

    captured_cargo.wasi_c_abi.path(missing).unlink()
    # Ordinary compilation trusts the append-only managed generation. Explicit
    # verification owns diagnosis of unsupported modifications to that generation.
    with pytest.raises((OSError, ValueError, LlvmToolchainConfigError)):
        load_wasi_sdk_installation(
            _compiler_root(), captured_cargo.wasi_sdk.prefix, verify_tree=True
        )


def test_runtime_plan_keeps_sdk_out_of_mutable_resource_capture(
    captured_cargo, monkeypatch
):
    sdk = captured_cargo.wasi_sdk.sdk
    assert not any(
        item.entrypoint.is_relative_to(sdk)
        for item in captured_cargo.rust_resources.files
    )
    assert not any(
        item.entrypoint.is_relative_to(sdk)
        for item in captured_cargo.executable_custody
    )
    with monkeypatch.context() as projection:
        projection.setattr(
            type(captured_cargo),
            "verify",
            lambda *_: pytest.fail("projection performed verification"),
        )
        projection.setattr(
            Path, "open", lambda *_a, **_k: pytest.fail("projection read a file")
        )
        captured_cargo.configuration_identity()
        captured_cargo.toolchain_identity()


def test_runtime_relink_requires_admitted_c_abi_before_effects(
    runtime_fixture_root, tmp_path, monkeypatch
):
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    plan = runtime_cargo_plan(
        tmp_path, fixture_root=runtime_fixture_root, env={}, cargo_command=("cargo",)
    )
    monkeypatch.setattr(
        rb,
        "_run_completed_command",
        lambda *_a, **_k: pytest.fail("link ran without SDK"),
    )
    with pytest.raises(ValueError, match="admitted SDK"):
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=tmp_path / "input.a",
            output_path=tmp_path / "out.wasm",
            json_output=True,
            link_timeout=1.0,
            cargo_plan=plan,
        )


@pytest.mark.parametrize(
    "resource",
    (
        "staticlib",
        "response",
    ),
)
def test_reloc_link_rejects_changed_inputs_without_replacing_output(
    runtime_fixture_root: RuntimeFixtureRoot,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    resource: str,
) -> None:
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"!<arch>\n")
    output = tmp_path / "runtime.wasm"
    output.write_bytes(b"old-publication")

    def mutate(command, **kwargs):
        assert kwargs["env"]["CAPTURED_LINK_ENV"] == "original"
        Path(command[-1]).write_bytes(b"\0asm\x01\0\0\0")
        if resource == "response":
            selected = Path(next(arg[1:] for arg in command if arg.startswith("@")))
        elif resource == "staticlib":
            selected = staticlib
        selected.write_bytes(selected.read_bytes() + b"changed")
        return subprocess.CompletedProcess(command, 0, "link-stdout", "link-stderr")

    monkeypatch.setattr(rb, "_run_completed_command", mutate)
    with pytest.raises(rb.RuntimeWasmLinkError, match="changed") as caught:
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=staticlib,
            output_path=output,
            json_output=True,
            link_timeout=1.0,
            cargo_plan=runtime_cargo_plan(
                tmp_path,
                fixture_root=runtime_fixture_root,
                env={"CAPTURED_LINK_ENV": "original"},
                cargo_command=("cargo",),
                requested_target="wasm32-wasip1",
            ),
            export_link_args="-C link-arg=--export=entry",
        )
    assert caught.value.stdout == "link-stdout"
    assert caught.value.stderr == "link-stderr"
    assert output.read_bytes() == b"old-publication"
    assert not list(tmp_path.glob(".molt-wasm-reloc-*.tmp"))


@pytest.mark.parametrize("timeout", (False, True))
def test_reloc_link_failure_retains_child_evidence(
    runtime_fixture_root: RuntimeFixtureRoot,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    timeout: bool,
) -> None:
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"!<arch>\n")
    output = tmp_path / "runtime.wasm"

    def fail(command, **_kwargs):
        if timeout:
            raise subprocess.TimeoutExpired(
                command, 1.0, output=b"partial stdout", stderr=b"precise cause"
            )
        return subprocess.CompletedProcess(
            command, 2, "partial stdout", "precise cause"
        )

    monkeypatch.setattr(rb, "_run_completed_command", fail)
    with pytest.raises(rb.RuntimeWasmLinkError) as caught:
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=staticlib,
            output_path=output,
            json_output=True,
            link_timeout=1.0,
            cargo_plan=runtime_cargo_plan(
                tmp_path,
                fixture_root=runtime_fixture_root,
                env={},
                cargo_command=("cargo",),
                requested_target="wasm32-wasip1",
            ),
        )
    from tests.runtime_build_identity_helper import runtime_wasi_c_abi_plan

    assert caught.value.command[0] == str(
        runtime_wasi_c_abi_plan(runtime_fixture_root).linker
    )
    assert caught.value.stdout == "partial stdout"
    assert caught.value.stderr == "precise cause"
    assert caught.value.timed_out is timeout
    assert not output.exists()


# Split app owns its formatter; the combined link already has the reloc one.
import wasm_link_native_inputs  # noqa: E402
from molt.cli.source_extension_link_requirements import (  # noqa: E402
    SourceExtensionLinkRequirements,
    source_extension_link_file,
)


def _split_native_requirements(*paths: Path) -> SourceExtensionLinkRequirements:
    return SourceExtensionLinkRequirements(
        "wasm32-wasip1", tuple(source_extension_link_file(path) for path in paths)
    )


def test_split_app_wholearchives_longdouble_when_libc_present(selected_c_abi) -> None:
    requirements = _split_native_requirements(
        selected_c_abi.path("libc"),
        selected_c_abi.path("long_double"),
        selected_c_abi.path("compiler_rt"),
    )
    args = wasm_link_native_inputs._split_app_native_link_args(
        requirements, provider_paths={"long_double": selected_c_abi.path("long_double")}
    )
    assert args == [
        "--whole-archive",
        str(selected_c_abi.path("long_double")),
        "--no-whole-archive",
        str(selected_c_abi.path("libc")),
        str(selected_c_abi.path("compiler_rt")),
    ]


def test_split_app_plain_passthrough_without_libc(tmp_path: Path) -> None:
    obj = tmp_path / "extmod.o"
    obj.write_bytes(b"\0asm\x01\0\0\0")
    requirements = _split_native_requirements(obj)
    assert wasm_link_native_inputs._split_app_native_link_args(
        requirements, provider_paths={}
    ) == [str(obj)]


def test_split_app_fails_before_link_when_longdouble_not_captured(
    selected_c_abi,
) -> None:
    with pytest.raises(ValueError, match="captured exactly once"):
        wasm_link_native_inputs._split_app_native_link_args(
            _split_native_requirements(selected_c_abi.path("libc")),
            provider_paths={"long_double": selected_c_abi.path("long_double")},
        )


def test_same_named_foreign_libc_cannot_join_the_selected_sdk(
    selected_c_abi, tmp_path: Path
) -> None:
    foreign = tmp_path / "libc.a"
    foreign.write_bytes(selected_c_abi.path("libc").read_bytes())
    with pytest.raises(ValueError, match="differs from selected WASI SDK"):
        wasm_link_inputs.admit_wasi_provider_inputs((foreign,))


def test_reloc_and_split_share_the_selected_complete_family(selected_c_abi) -> None:
    policy = wasm_link_inputs.resolve_long_double_link_policy()
    assert (policy.printscan, policy.builtins) == (
        selected_c_abi.path("long_double"),
        selected_c_abi.path("compiler_rt"),
    )
    argv = wasm_link_inputs.long_double_whole_archive_link_argv(
        policy,
        whole_archive=["staticlib.a"],
        trailing=[str(selected_c_abi.path("libc"))],
    )
    assert argv == [
        "--whole-archive",
        "staticlib.a",
        str(selected_c_abi.path("long_double")),
        "--no-whole-archive",
        str(selected_c_abi.path("libc")),
        str(selected_c_abi.path("compiler_rt")),
    ]


def test_shared_and_reloc_families_attest_exact_archive_content(tmp_path: Path) -> None:
    from molt.cli.runtime_identity_schema import (
        RuntimeBuildIdentity,
        _digest,
    )
    from tests.runtime_build_identity_helper import runtime_build_identity

    archive = tmp_path / "libc-printscan-long-double.a"
    archive.write_bytes(b"!<arch>\nfirst")
    before = runtime_build_identity("shared").to_dict()

    def with_archive(value):
        family = value["payload"]["family"]
        archives = family["compile"]["toolchain"]["archives"]
        raw = archive.read_bytes()
        archives[1] = {
            "logical_name": "wasi-long-double",
            "size": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
        }
        compile_digest = _digest(family["compile"])
        family["compile_digest"] = compile_digest
        return tuple(
            RuntimeBuildIdentity(
                _digest({"family": family, "member_kind": kind}),
                compile_digest,
                _digest(family),
                {"family": family, "member_kind": kind},
            )
            for kind in ("shared", "reloc")
        )

    first = with_archive(before)
    archive.write_bytes(b"!<arch>\nother")
    second = with_archive(before)
    assert first[0].family_digest == first[1].family_digest
    assert second[0].family_digest == second[1].family_digest
    assert all(
        old.compile_digest != new.compile_digest and old.digest != new.digest
        for old, new in zip(first, second, strict=True)
    )


def test_python_projection_is_consumed_by_actual_rust_decoder(
    runtime_fixture_root, tmp_path
):
    import os
    import shutil
    from tests.runtime_build_identity_helper import runtime_wasi_c_abi_plan
    from tests.process_guard_common import run_guarded_test_process
    from molt.source_root import compiler_source_root
    from molt.cli.wasm_link_args import wasi_external_libc_rustflags

    rustc = shutil.which("rustc")
    assert rustc is not None, "Rust decoder proof requires admitted rustc"
    plan = runtime_wasi_c_abi_plan(runtime_fixture_root)
    source = tmp_path / "decoder.rs"
    decoder = compiler_source_root() / "runtime/build_support/wasi_sysroot.rs"
    source.write_text(
        "#[path = " + __import__("json").dumps(str(decoder)) + "] mod wasi;\n"
        "fn main() { let a: Vec<String> = std::env::args().collect();\n"
        ' let plan = wasi::WasiCAbiPlan::decode(&a[1]).expect("decode");\n'
        ' plan.validate_cargo_mode(&a[4], &a[2], &a[3]).expect("Cargo mode"); }\n',
        encoding="utf-8",
    )
    executable = tmp_path / ("decoder.exe" if os.name == "nt" else "decoder")
    run_guarded_test_process(
        [rustc, "--edition=2024", str(source), "-o", str(executable)],
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    # The Rust decoder accepting our spelling is insufficient: the selected
    # stable compiler must accept the production producer's complete mode.
    produced = wasi_external_libc_rustflags((), plan=plan)
    assert produced == (
        "-L",
        "native=" + str(plan.path("libc").parent),
        "-L",
        "native=" + str(plan.path("compiler_rt").parent),
        "-C",
        "link-self-contained=no",
        "-C",
        "linker-flavor=wasm-ld",
    )
    flags = "\x1f".join(produced)
    environment = {
        key: value for key, value in os.environ.items() if key != "RUSTC_BOOTSTRAP"
    }
    for target in ("wasm32-wasip1", "wasm32-unknown-unknown"):
        cfg = run_guarded_test_process(
            [rustc, "--print", "cfg", "--target", target, *produced],
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        assert 'target_arch="wasm32"' in cfg.stdout.splitlines()
        valid = [str(executable), plan.encode(), str(plan.linker), flags, target]
        run_guarded_test_process(
            valid, capture_output=True, text=True, check=True, timeout=10
        )
    # The receiver sees raw effective Cargo flags, not necessarily our Python
    # canonical spelling. Exercise both aliases and each conflicting mode there.
    for spelling in ("-C", "-Cjoined", "--codegen", "--codegen="):

        def option(value):
            return (
                (spelling, value)
                if spelling in {"-C", "--codegen"}
                else (("-C" if spelling == "-Cjoined" else spelling) + value,)
            )

        modes = (*option("link-self-contained=no"), *option("linker-flavor=wasm-ld"))
        raw = (*produced[:4], *modes)
        run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join(raw),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        for conflicting in (
            "link-self-contained=yes",
            "linker-flavor=wasm-lld-cc",
            "linker=foreign-linker",
        ):
            result = run_guarded_test_process(
                [
                    str(executable),
                    plan.encode(),
                    str(plan.linker),
                    "\x1f".join((*raw, *option(conflicting))),
                    "wasm32-wasip1",
                ],
                capture_output=True,
                text=True,
                timeout=10,
            )
            assert result.returncode != 0
    for switch in ("--sysroot", "-L", "-o", "--out-dir", "--remap-path-prefix"):
        for opaque in ("-Clinker=foreign", "--codegen=linker=foreign"):
            run_guarded_test_process(
                [
                    str(executable),
                    plan.encode(),
                    str(plan.linker),
                    "\x1f".join((*produced, switch, opaque)),
                    "wasm32-wasip1",
                ],
                capture_output=True,
                text=True,
                check=True,
                timeout=10,
            )
    for output in ("-C", "-Lnative=unselected", "--codegen"):
        run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join(("-o", output, *produced)),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
    clustered = (
        *produced[:4],
        "-gClink_self_contained=no",
        "-vC",
        "linker_flavor=wasm-ld",
    )
    run_guarded_test_process(
        [
            str(executable),
            plan.encode(),
            str(plan.linker),
            "\x1f".join(clustered),
            "wasm32-wasip1",
        ],
        capture_output=True,
        text=True,
        check=True,
        timeout=10,
    )
    for feature, expected in (("+simd128", True), ("-simd128", False)):
        cfg = run_guarded_test_process(
            [
                rustc,
                "--print",
                "cfg",
                "--target",
                "wasm32-wasip1",
                *clustered,
                "-gCtarget_feature=" + feature,
            ],
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        assert ('target_feature="simd128"' in cfg.stdout.splitlines()) is expected
    for conflict in (
        "-gClink_self_contained=yes",
        "-OClinker_flavor=wasm-lld-cc",
        "-gClinker=foreign",
    ):
        result = run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join((*clustered, conflict)),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.returncode != 0
    for search in (
        ("-L", "unknown=archive"),
        ("-gLunknown=archive",),
        ("-L", "-Clinker=tools/root"),
    ):
        preceding = run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join((*search, *produced)),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert (
            preceding.returncode != 0
            and "selected SDK directories first" in preceding.stderr
        )
        run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join((*produced, *search)),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
    for malformed in (("-C",), ("--codegen",), ("--codegen=",), ("-C=opt-level=2",)):
        result = run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join((*produced, *malformed)),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.returncode != 0

    # Cargo global flags supersede target flags. The direct environment writer
    # preserves that native caller input, and the real Rust decoder refuses the
    # now-incomplete WASI effective lane rather than silently supplementing it.
    from molt.llvm_toolchain import project_wasm_toolchain_environment
    from tests.runtime_build_identity_helper import provisioned_wasi_sdk_fixture

    installation = provisioned_wasi_sdk_fixture(runtime_fixture_root)
    for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        incomplete = produced[4:]
        value = (
            "\x1f".join(incomplete)
            if name == "CARGO_ENCODED_RUSTFLAGS"
            else __import__("shlex").join(incomplete)
        )
        projected = project_wasm_toolchain_environment(
            installation, environ={name: value}
        )
        assert projected[name] == value  # the writer never rewrites global flags
        assert __import__("shlex").split(
            projected["CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS"]
        ) == list(produced)
        result = run_guarded_test_process(
            [
                str(executable),
                plan.encode(),
                str(plan.linker),
                "\x1f".join(incomplete),
                "wasm32-wasip1",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert (
            result.returncode != 0 and "selected SDK directories first" in result.stderr
        )

    old = bytes.fromhex(plan.encode()).decode().split("\0")
    old[0] = "molt.wasi-c-abi.v1"
    del old[10]
    for wire, linker, mode in (
        ("\0".join(old).encode().hex(), str(plan.linker), flags),
        (plan.encode(), str(plan.driver), flags),
        (plan.encode(), str(plan.linker), flags.replace("wasm-ld", "wasm-lld")),
        (plan.encode(), str(plan.linker), flags.replace("wasm-ld", "wasm-lld-cc")),
        (
            plan.encode(),
            str(plan.linker),
            flags.replace("contained=no", "contained=yes"),
        ),
        (plan.encode(), str(plan.linker), "\x1f".join(produced[4:])),
        (
            plan.encode(),
            str(plan.linker),
            "\x1f".join((*produced[2:4], *produced[:2], *produced[4:])),
        ),
    ):
        for target in ("wasm32-wasip1", "wasm32-unknown-unknown"):
            result = run_guarded_test_process(
                [str(executable), wire, linker, mode, target],
                capture_output=True,
                text=True,
                timeout=10,
            )
            assert result.returncode != 0


def test_split_renderer_uses_captured_role_paths_without_live_sdk(
    selected_c_abi, tmp_path, monkeypatch
):
    import shutil

    captured = {}
    for role, source in (
        ("libc", selected_c_abi.path("libc")),
        ("long_double", selected_c_abi.path("long_double")),
        ("compiler_rt", selected_c_abi.path("compiler_rt")),
    ):
        destination = tmp_path / ("snapshot-" + role + ".a")
        shutil.copyfile(source, destination)
        captured[role] = destination
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **kwargs: pytest.fail("renderer selected live SDK"),
    )
    arguments = wasm_link_native_inputs._split_app_native_link_args(
        _split_native_requirements(*captured.values()),
        provider_paths=captured,
    )
    assert arguments == [
        "--whole-archive",
        str(captured["long_double"]),
        "--no-whole-archive",
        str(captured["libc"]),
        str(captured["compiler_rt"]),
    ]


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
def test_wasi_codegen_aliases_share_mode_and_raw_argument_admission(
    selected_c_abi, spelling
):
    import shlex
    from molt.cli.wasm_link_args import (
        wasi_external_libc_rustflags,
        wasm_link_args_from_rustflags,
    )

    def option(value):
        return (
            (spelling, value)
            if spelling in {"-C", "--codegen"}
            else (("-C" if spelling == "-Cjoined" else spelling) + value,)
        )

    original = (
        *option("link-self-contained=no"),
        *option("linker-flavor=wasm-ld"),
        *option("link-arg=--export-if-defined=molt_entry_point"),
    )
    normalized = wasi_external_libc_rustflags(original, plan=selected_c_abi)
    assert normalized[4:] == (
        "-C",
        "link-self-contained=no",
        "-C",
        "linker-flavor=wasm-ld",
        "-C",
        "link-arg=--export-if-defined=molt_entry_point",
    )
    assert wasi_external_libc_rustflags(normalized, plan=selected_c_abi) == normalized
    assert wasm_link_args_from_rustflags(shlex.join(original)) == [
        "--export-if-defined=molt_entry_point"
    ]
    assert wasm_link_args_from_rustflags(
        shlex.join((*original, "--", *option("link-arg=positional")))
    ) == ["--export-if-defined=molt_entry_point"]
    for conflict in ("link-self-contained=yes", "linker-flavor=wasm-lld-cc"):
        with pytest.raises(ValueError, match="external-libc mode conflicts"):
            wasi_external_libc_rustflags(option(conflict), plan=selected_c_abi)
    with pytest.raises(ValueError, match="unsupported|resource custody"):
        wasi_external_libc_rustflags(
            option("link-arg=--sysroot=foreign"), plan=selected_c_abi
        )
    with pytest.raises(ValueError, match="individually admitted"):
        wasi_external_libc_rustflags(
            option("link-args=--export=one --export=two"), plan=selected_c_abi
        )


@pytest.mark.parametrize(
    "switch", ["--sysroot", "-o", "--out-dir", "--remap-path-prefix"]
)
def test_wasi_projection_preserves_opaque_codegen_looking_operand(
    selected_c_abi, switch
):
    from molt.cli.wasm_link_args import wasi_external_libc_rustflags

    flags = (switch, "--codegen=linker-flavor=wasm-lld-cc")
    normalized = wasi_external_libc_rustflags(flags, plan=selected_c_abi)
    assert normalized[4:6] == flags
    assert normalized[6:] == (
        "-C",
        "link-self-contained=no",
        "-C",
        "linker-flavor=wasm-ld",
    )
    assert wasi_external_libc_rustflags(normalized, plan=selected_c_abi) == normalized


@pytest.mark.parametrize("output", ["-C", "-Lnative=unselected", "--codegen"])
def test_wasi_search_projection_does_not_rescan_output_operand(selected_c_abi, output):
    from molt.cli.wasm_link_args import wasi_external_libc_rustflags

    flags = ("-o", output, "--codegen=panic=abort")
    normalized = wasi_external_libc_rustflags(flags, plan=selected_c_abi)
    assert normalized[4:8] == ("-o", output, "-C", "panic=abort")
    assert wasi_external_libc_rustflags(normalized, plan=selected_c_abi) == normalized


@pytest.mark.parametrize(
    "conflict", ["-gClink_self_contained=yes", "-vClinker_flavor=wasm-lld-cc"]
)
def test_wasi_clustered_key_aliases_cannot_bypass_required_mode(
    selected_c_abi, conflict
):
    from molt.cli.wasm_link_args import wasi_external_libc_rustflags

    with pytest.raises(ValueError, match="external-libc mode conflicts"):
        wasi_external_libc_rustflags((conflict,), plan=selected_c_abi)


def test_wasi_clustered_projection_preserves_prefix_and_key_value_boundary(
    selected_c_abi,
):
    import shlex
    from molt.cli.wasm_link_args import (
        wasi_external_libc_rustflags,
        wasm_link_args_from_rustflags,
    )

    flags = (
        "-gClink_self_contained=no",
        "-vC",
        "linker_flavor=wasm-ld",
        "--codegen=link_arg=--export=under_score",
    )
    normalized = wasi_external_libc_rustflags(flags, plan=selected_c_abi)
    assert normalized[4:] == (
        "-g",
        "-C",
        "link-self-contained=no",
        "-v",
        "-C",
        "linker-flavor=wasm-ld",
        "-C",
        "link-arg=--export=under_score",
    )
    assert wasi_external_libc_rustflags(normalized, plan=selected_c_abi) == normalized
    assert wasm_link_args_from_rustflags(shlex.join(flags)) == ["--export=under_score"]
    with pytest.raises(ValueError, match="individually admitted"):
        wasi_external_libc_rustflags(
            ("-gClink_args=--export=one --export=two",), plan=selected_c_abi
        )
    with pytest.raises(ValueError, match="external-libc mode conflicts"):
        wasi_external_libc_rustflags(
            (*flags, "-C", "link-self-contained=no"), plan=selected_c_abi
        )

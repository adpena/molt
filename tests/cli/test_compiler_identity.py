from __future__ import annotations

import json
import hashlib
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import backend_binary, backend_compile, backend_execution
from molt.cli import cache_fingerprints, cargo_source_closure, compiler_identity
from molt.cli import wrapper_build
from molt.exact_json import canonical_json_sha256
from tests.compiler_identity_helper import compiler_build_admission, write_compiler_lock


def lock_identity(root: Path) -> str:
    return cargo_source_closure._cargo_locked_dependency_digest(
        root, root / "runtime/molt-backend"
    )


def test_unreachable_bad_rows_do_not_become_a_second_lock_validator(tmp_path):
    write_compiler_lock(tmp_path)
    baseline = lock_identity(tmp_path)
    with (tmp_path / "Cargo.lock").open("a") as stream:
        stream.write(
            '\n[[package]]\nname="unrelated"\nversion="0.1.0"\nchecksum="invalid"\ndependencies=["missing", "broken ("]\n'
        )
    assert lock_identity(tmp_path) == baseline
    text = (tmp_path / "Cargo.lock").read_text(encoding="utf-8")
    text = text.replace(
        'version = "0.1.0"', 'version = "0.1.0"\ndependencies=["unrelated"]', 1
    )
    (tmp_path / "Cargo.lock").write_text(text, encoding="utf-8")
    with pytest.raises(compiler_identity.CompilerIdentityError, match="checksum"):
        lock_identity(tmp_path)


@pytest.mark.parametrize(
    "reference,source",
    [
        (
            "dep 1.0.0 (git+https://example.invalid/repo?rev=main)",
            "git+https://example.invalid/repo?rev=main#abc",
        ),
        ("dep 1.0.0", ""),
    ],
)
def test_cargo_git_reference_and_path_precedence(tmp_path, reference, source):
    write_compiler_lock(tmp_path)
    root = 'version=4\n[[package]]\nname="molt-backend"\nversion="0.1.0"\n'
    selected = '\n[[package]]\nname="dep"\nversion="1.0.0"\n'
    if source:
        selected += f'source="{source}"\n'
    unrelated = (
        '\n[[package]]\nname="dep"\nversion="1.0.0"\nsource="registry+https://example.invalid/index"\nchecksum="'
        + "a" * 64
        + '"\n'
    )
    lock = tmp_path / "Cargo.lock"
    text = (
        root + "dependencies=[" + json.dumps(reference) + "]\n" + selected + unrelated
    )
    lock.write_text(text, encoding="utf-8")
    baseline = lock_identity(tmp_path)
    lock.write_text(text.replace("a" * 64, "b" * 64), encoding="utf-8")
    assert lock_identity(tmp_path) == baseline
    if source:
        lock.write_text(text.replace("#abc", "#def"), encoding="utf-8")
    else:
        lock.write_text(
            text.replace(selected, selected + 'dependencies=["extra"]\n')
            + '\n[[package]]\nname="extra"\nversion="1.0.0"\n',
            encoding="utf-8",
        )
    assert lock_identity(tmp_path) != baseline


@pytest.mark.parametrize("command", ["run", "deploy"])
@pytest.mark.parametrize("stage", ["before", "after"])
def test_wrapper_converts_compiler_identity_rejection(
    tmp_path, monkeypatch, capsys, command, stage
):
    entry = SimpleNamespace(
        image_scope="project", target_python="3.12", entry_source="source"
    )
    monkeypatch.setattr(
        wrapper_build._build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kw: (entry, None),
    )
    calls = 0

    def reject(**kw):
        nonlocal calls
        calls += 1
        if stage == "after" and calls == 1:
            return {}, "initial"
        raise compiler_identity.CompilerIdentityError("reachable dependency is missing")

    monkeypatch.setattr(wrapper_build, "_wrapper_build_cache_input", reject)
    monkeypatch.setattr(
        wrapper_build, "_read_wrapper_build_cache_contract", lambda **kw: None
    )
    monkeypatch.setattr(
        wrapper_build,
        "_run_completed_command",
        lambda command, **kw: subprocess.CompletedProcess(command, 0, "{}", ""),
    )
    monkeypatch.setattr(
        wrapper_build,
        "_parse_wrapper_build_contract_payload",
        lambda *args, **kw: (SimpleNamespace(), None),
    )
    result = wrapper_build._run_wrapper_build(
        file_path=str(tmp_path / "app.py"),
        module=None,
        build_args=[],
        env={},
        project_root=tmp_path,
        json_output=True,
        command=command,
        verbose=False,
        resolved_build_entry=entry,
    )
    assert result[0] is None and result[2] is not None
    output = capsys.readouterr().out
    assert "reachable dependency is missing" in output
    assert "Traceback" not in output


def test_daemon_dispatch_converts_compiler_identity_rejection(
    tmp_path, monkeypatch, capsys
):
    binary = tmp_path / "backend"
    binary.write_bytes(b"compiler")
    monkeypatch.setattr(backend_compile, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(
        backend_compile,
        "native_runtime_codegen_environment",
        lambda env, binding: dict(env),
    )

    def reject(*args, **kw):
        raise compiler_identity.CompilerIdentityError("compiler inputs changed")

    monkeypatch.setattr(backend_compile, "_backend_daemon_config_digest", reject)
    prepared, error = backend_compile._prepare_backend_dispatch(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        wasm_layout=None,
        deterministic=True,
        profile="dev",
        cargo_timeout=1,
        molt_root=tmp_path,
        target_triple=None,
        backend_cargo_profile="release",
        diagnostics_enabled=False,
        phase_starts={},
        json_output=True,
        backend_daemon_config_digest=None,
        warnings=[],
        backend_bin=binary,
        backend_compiler_fingerprint="admitted",
        native_runtime_codegen_binding=object(),
    )
    assert prepared is None and error is not None
    assert "compiler inputs changed" in capsys.readouterr().out


@pytest.mark.parametrize(
    "mutation",
    [
        "source",
        "lock",
        "configuration",
        "restored-source",
        "restored-lock",
        "restored-topology",
    ],
)
def test_cargo_mutation_rejected_before_any_alias_probe_or_receipt(
    tmp_path, monkeypatch, mutation
):
    write_compiler_lock(tmp_path)
    source = tmp_path / "runtime/molt-backend/lib.rs"
    source.write_text("pub const VALUE: u8 = 1;\n", encoding="utf-8")
    admission = compiler_build_admission(("native-backend",), "release")
    built = False

    def verify():
        if built and mutation == "configuration":
            raise compiler_identity.CompilerIdentityError("Cargo configuration changed")

    admission.verify = verify
    monkeypatch.setattr(
        backend_binary, "backend_build_admission", lambda *args: admission
    )
    monkeypatch.setattr(
        backend_binary, "_backend_source_paths", lambda *args: [source.parent]
    )
    monkeypatch.setattr(
        backend_binary, "_compiler_clean_source_state", lambda *args: None
    )
    monkeypatch.setattr(
        backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **kw: False,
    )
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)

    def cargo(plan, **kw):
        nonlocal built
        assert "--locked" in plan.command
        built = True
        if mutation == "source":
            source.write_text("pub const VALUE: u8 = 2;\n", encoding="utf-8")
        elif mutation in {"restored-source", "restored-lock"}:
            changed = (
                source if mutation == "restored-source" else tmp_path / "Cargo.lock"
            )
            original, metadata = changed.read_bytes(), changed.stat()
            changed.write_bytes(b"other-generation")
            changed.write_bytes(original)
            os.utime(changed, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        elif mutation == "restored-topology":
            extra = source.parent / "during-cargo.rs"
            extra.write_text("pub const TEMP: u8 = 1;", encoding="utf-8")
            extra.unlink()
        elif mutation == "lock":
            lock = tmp_path / "Cargo.lock"
            lock.write_text(
                lock.read_text(encoding="utf-8").replace(
                    'version = "0.1.0"', 'version = "0.2.0"'
                ),
                encoding="utf-8",
            )
        return subprocess.CompletedProcess(plan.command, 0, "", "")

    monkeypatch.setattr(backend_binary, "_run_resolved_cargo_plan", cargo)

    def forbidden(*args, **kw):
        raise AssertionError("unstable build must not publish or probe")

    for name in (
        "_atomic_copy_file",
        "_run_subprocess_captured_to_tempfiles",
        "_write_runtime_fingerprint",
        "_atomic_write_json",
    ):
        monkeypatch.setattr(backend_binary, name, forbidden)
    result = backend_binary._ensure_backend_binary(
        tmp_path / "target/release/backend.native",
        cargo_timeout=1,
        json_output=True,
        cargo_profile="release",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert built and not result.ok and result.phase == "backend_source_identity"


def test_next_operation_observes_clean_tree_edit(tmp_path, monkeypatch):
    from molt.cli import compiler_metadata

    results = iter(["# branch.oid abc\n", "# branch.oid abc\n? edited.rs\n"])
    monkeypatch.setattr(
        compiler_metadata,
        "_run_completed_command",
        lambda command, **kw: subprocess.CompletedProcess(
            command, 0, next(results), ""
        ),
    )
    assert compiler_metadata._compiler_clean_source_state(tmp_path) is not None
    assert compiler_metadata._compiler_clean_source_state(tmp_path) is None


def test_daemon_identity_uses_admitted_compiler_and_request_environment(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(
        backend_execution, "_cache_tooling_fingerprint", lambda: "tooling"
    )
    monkeypatch.setattr(
        backend_execution,
        "_cache_fingerprint",
        lambda **kw: pytest.fail("compiler already admitted"),
    )
    first = backend_execution._backend_daemon_config_digest(
        tmp_path, "release", env={"MOLT_BACKEND_COMPILER_FINGERPRINT": "a" * 64}
    )
    second = backend_execution._backend_daemon_config_digest(
        tmp_path, "release", env={"MOLT_BACKEND_COMPILER_FINGERPRINT": "b" * 64}
    )
    assert first != second


def test_prepared_cargo_environment_and_configuration_govern_identity(
    tmp_path, monkeypatch
):
    from molt.cli import cargo_execution, runtime_cargo_plan

    seen = []
    configuration = tmp_path / "config.toml"
    configuration.write_text('[build]\nrustflags=["-Copt-level=2"]\n', encoding="utf-8")
    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))

    def resolve(root, *, env, cargo_command, requested_target, environment_transform):
        env = environment_transform(env)
        seen.append(dict(env))
        config_digest = canonical_json_sha256(configuration.read_text(encoding="utf-8"))
        return SimpleNamespace(
            environment=env,
            configuration=(),
            cli_configuration={},
            rustflags=(env.get("CARGO_BUILD_RUSTFLAGS", ""),),
            c_environment={},
            partition_command=lambda: (tuple(cargo_command), ()),
            link_resources=SimpleNamespace(content_identity=lambda: {}),
            profile_environment=lambda profile: {
                key: value
                for key, value in env.items()
                if key.startswith("CARGO_PROFILE_")
            },
            config_digest=config_digest,
            toolchain_identity=lambda: {"config": config_digest},
            verify=lambda: None,
        )

    monkeypatch.setattr(runtime_cargo_plan, "resolve_runtime_cargo_plan", resolve)
    baseline_env = {
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "MOLT_NATIVE_CPU": "1",
    }

    def admission(env):
        return compiler_identity.backend_build_admission(
            tmp_path, ("native-backend",), "release", env
        )

    with cache_fingerprints._source_tree_fingerprint_transaction():
        baseline = admission(baseline_env)
        assert admission(baseline_env) is baseline
        assert (
            len(seen) == 1 and "target-cpu=native" in seen[0]["CARGO_BUILD_RUSTFLAGS"]
        )
    profile = admission({**baseline_env, "CARGO_PROFILE_RELEASE_LTO": "true"})
    assert profile.fingerprint != baseline.fingerprint
    # Public Luau builds choose guest chunking after a prewarm. Neither this
    # setting nor diagnostic verbosity changes the admitted host compiler.
    guest = admission(
        {
            **baseline_env,
            "MOLT_MODULE_CHUNK_OPS": "1500",
            "MOLT_BUILD_DIAGNOSTICS_VERBOSITY": "verbose",
        }
    )
    assert guest.fingerprint == baseline.fingerprint
    configuration.write_text('[build]\nrustflags=["-Copt-level=3"]\n', encoding="utf-8")
    assert admission(baseline_env).fingerprint != baseline.fingerprint


def test_lock_projection_is_mandatory_without_raw_lock_in_source_paths(tmp_path):
    write_compiler_lock(tmp_path)
    paths, digest = cache_fingerprints._backend_source_identity_inputs(tmp_path, [])
    assert paths == [] and digest == lock_identity(tmp_path)
    (tmp_path / "Cargo.lock").unlink()
    with pytest.raises(compiler_identity.CompilerIdentityError):
        cache_fingerprints._backend_source_identity_inputs(tmp_path, [])


@pytest.mark.parametrize("mutate_during_admission", [False, True])
def test_llvm_prefix_bytes_belong_to_selected_compiler_identity(
    tmp_path, monkeypatch, mutate_during_admission
):
    from molt import llvm_toolchain
    from molt.cli import cargo_execution, runtime_cargo_plan

    prefix = tmp_path / "llvm"
    library = prefix / "lib" / "libLLVM.a"
    header = prefix / "include" / "llvm-c" / "Core.h"
    unrelated = prefix / "lib" / "python3.12" / "site-packages" / "lldb" / "lldb.py"
    llvm_config = prefix / "bin" / "llvm-config"
    for path, content in (
        (library, b"LLVM-one"),
        (header, b"/* core */"),
        (unrelated, b"# lldb"),
        (llvm_config, b"llvm-config"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda root, **kw: SimpleNamespace(
            prefix=prefix,
            llvm_config=llvm_config,
            link_closure=("lib/libLLVM.a",),
            tool_versions=(
                llvm_toolchain.LlvmToolVersionFact(
                    "llvm-config",
                    "bin/llvm-config",
                    "22.1.8",
                    len(b"llvm-config"),
                    hashlib.sha256(b"llvm-config").hexdigest(),
                ),
            ),
            content_facts=(),
            library_facts=(
                llvm_toolchain.LlvmLibraryFact(
                    path="lib/libLLVM.a", size=8, mtime_ns=0
                ),
            ),
        ),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "project_llvm_toolchain_environment",
        lambda root, verification, **kw: dict(kw["environ"]),
    )
    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))
    # The fake Cargo plan has no mutable resource inputs of its own. LLVM
    # resources below are captured and verified by the real admission owner.
    cargo_custody = runtime_cargo_plan.CargoResourceCustody.capture(())

    def project_tools():
        if mutate_during_admission:
            header.write_bytes(b"/* changed after resource capture */")
        return {}

    monkeypatch.setattr(
        runtime_cargo_plan,
        "resolve_runtime_cargo_plan",
        lambda root, **kw: SimpleNamespace(
            environment=kw["environment_transform"](kw["env"]),
            configuration=(),
            cli_configuration={},
            profile_environment=lambda profile: {},
            rustflags=(),
            c_environment={},
            toolchain_identity=project_tools,
            verify=cargo_custody.verify,
            partition_command=lambda: (kw["cargo_command"], ()),
            link_resources=SimpleNamespace(content_identity=lambda: {}),
        ),
    )
    env = {
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "LLVM_SYS_221_PREFIX": str(prefix),
    }

    def fingerprint():
        return compiler_identity.backend_build_admission(
            tmp_path, ("llvm", "native-backend"), "release", env
        ).fingerprint

    def rewrite(path, content):
        metadata = path.stat()
        path.write_bytes(content)
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))

    if mutate_during_admission:
        with cache_fingerprints._source_tree_fingerprint_transaction():
            with pytest.raises(
                compiler_identity.CompilerIdentityError, match="inputs changed"
            ):
                fingerprint()
            transaction = cache_fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
            assert transaction is not None and transaction.compiler_plans == {}
        return

    before = fingerprint()
    rewrite(library, b"LLVM-two")
    after_library = fingerprint()
    assert after_library != before
    rewrite(header, b"/* core, revised */")
    after_header = fingerprint()
    assert after_header != after_library
    # Only the consumed content set (headers plus attested link-closure
    # libraries) is a build input; the rest of the install is not.
    rewrite(unrelated, b"# lldb, revised")
    assert fingerprint() == after_header


def test_compiler_cargo_projection_never_admits_runtime_build_python(
    tmp_path, monkeypatch
):
    from molt.cli import cargo_execution, runtime_cargo_plan, runtime_build_identity

    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))
    monkeypatch.setattr(
        runtime_build_identity,
        "_python_identity",
        lambda *args, **kw: pytest.fail(
            "compiler does not execute runtime Python generators"
        ),
    )
    monkeypatch.setattr(
        runtime_cargo_plan,
        "resolve_runtime_cargo_plan",
        lambda root, **kw: SimpleNamespace(
            environment=kw["environment_transform"](kw["env"]),
            configuration=(),
            cli_configuration={},
            profile_environment=lambda profile: {},
            rustflags=(),
            c_environment={},
            partition_command=lambda: (kw["cargo_command"], ()),
            toolchain_identity=lambda: {"tools": {"rustc": "content"}},
            verify=runtime_cargo_plan.CargoResourceCustody.capture(()).verify,
            link_resources=SimpleNamespace(content_identity=lambda: {}),
        ),
    )

    def key(python):
        return compiler_identity.backend_build_admission(
            tmp_path,
            ("native-backend",),
            "release",
            {"CARGO_TARGET_DIR": str(tmp_path / "target"), "MOLT_BUILD_PYTHON": python},
        ).fingerprint

    assert key("missing-runtime-python-a") == key("missing-runtime-python-b")


def test_runtime_augments_shared_cargo_projection_with_required_python(monkeypatch):
    from molt.cli import runtime_build_identity, runtime_cargo_plan

    calls = []

    def python(env, *, admission):
        calls.append("python")
        return {"identity": env["MOLT_BUILD_PYTHON"]}

    def cargo():
        calls.append("cargo-custody")
        return {"tools": {"rustc": "content"}, "cargo_configuration": "config"}

    cargo_custody = runtime_cargo_plan.CargoResourceCustody.capture(())

    def verify():
        calls.append("cargo-verify")
        cargo_custody.verify()

    monkeypatch.setattr(runtime_build_identity, "_python_identity", python)
    result = runtime_build_identity._capture_plan_toolchain(
        SimpleNamespace(
            environment={"MOLT_BUILD_PYTHON": "runtime-python"},
            toolchain_identity=cargo,
            verify=verify,
        )
    )
    assert calls == ["python", "cargo-verify", "cargo-custody"]
    assert result["tools"]["build_python"] == {"identity": "runtime-python"}
    assert result["tools"]["rustc"] == "content"

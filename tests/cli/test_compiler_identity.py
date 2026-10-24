from __future__ import annotations

from molt.process_environment import ProcessEnvironment
from molt.compilation_target import TargetKind

import json
import hashlib
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.backend_environment import CodegenSelection
from molt.cli import backend_binary, backend_compile, backend_execution
from molt.cli import cache_fingerprints, cargo_resource_custody, cargo_source_closure
from molt.cli import compiler_identity
from molt.cli import wrapper_build
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.exact_json import canonical_json_sha256
from molt.host_tool_products import HostToolProduct
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

    def resolve_entry(*, request, **kw):
        assert request.target is TargetKind.NATIVE
        return entry, None

    monkeypatch.setattr(
        wrapper_build._build_inputs, "_resolve_request_entry", resolve_entry
    )
    calls = 0
    launches = []

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

    def launch(command, **kw):
        launches.append(command)
        return subprocess.CompletedProcess(command, 0, "{}", "")

    monkeypatch.setattr(wrapper_build, "_run_completed_command", launch)
    monkeypatch.setattr(
        wrapper_build,
        "_parse_wrapper_build_contract_payload",
        lambda *args, **kw: (SimpleNamespace(), None),
    )
    result = wrapper_build._run_wrapper_build(
        file_path=str(tmp_path / "app.py"),
        module=None,
        build_args=[],
        env=ProcessEnvironment({}),
        project_root=tmp_path,
        json_output=True,
        command=command,
        verbose=False,
        resolved_build_entry=entry,
    )
    assert result[0] is None and result[2] is not None
    assert calls == (1 if stage == "before" else 2)
    assert len(launches) == (0 if stage == "before" else 1)
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
        backend_environment=CodegenSelection().environment(os.environ, profile="dev"),
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
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
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

    def admit(
        root,
        features,
        profile,
        environment,
        *,
        rust_cpu,
        product=HostToolProduct.BACKEND,
    ):
        del environment
        assert root == tmp_path
        assert features == ("native-backend",)
        assert profile == "release"
        assert rust_cpu is RustCpuPolicy.BASELINE
        assert product is HostToolProduct.BACKEND
        return admission

    monkeypatch.setattr(backend_binary, "backend_build_admission", admit)

    def source_paths(root, features, *, product=HostToolProduct.BACKEND):
        assert root == tmp_path
        assert features == ("native-backend",)
        assert product is HostToolProduct.BACKEND
        return [source.parent]

    monkeypatch.setattr(backend_binary, "_backend_source_paths", source_paths)
    monkeypatch.setattr(
        backend_binary, "_compiler_clean_source_state", lambda *args: None
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
        "publish_backend_generation",
        "_run_subprocess_captured_to_tempfiles",
        "atomic_write_bytes",
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
        rust_cpu=RustCpuPolicy.BASELINE,
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


def test_daemon_identity_uses_the_admitted_compiler(tmp_path, monkeypatch):
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


@pytest.mark.parametrize("product", list(HostToolProduct))
def test_prepared_cargo_environment_and_configuration_govern_identity(
    tmp_path, monkeypatch, product
):
    from molt.cli import cargo_execution, runtime_cargo_plan

    seen = []
    configuration = tmp_path / "config.toml"
    configuration.write_text('[build]\nrustflags=["-Copt-level=2"]\n', encoding="utf-8")
    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))

    def resolve(
        root,
        *,
        env,
        cargo_command,
        requested_target,
        environment_transform,
        rustflags_transform,
    ):
        assert "--message-format=json-render-diagnostics" in cargo_command
        assert cargo_command[cargo_command.index("--package") + 1] == product.value
        assert cargo_command[cargo_command.index("--bin") + 1] == product.value
        env = environment_transform(env)
        seen.append(dict(env))
        config_digest = canonical_json_sha256(configuration.read_text(encoding="utf-8"))
        # Cargo selects the configured build flags; the plan pins the policy's
        # result exactly as the real resolver does.
        rustflags = (
            *rustflags_transform(("-Copt-level=2",)),
            f"--remap-path-prefix={root}=/molt/source",
            f"--remap-path-prefix={env['CARGO_TARGET_DIR']}=/molt/build",
        )
        return SimpleNamespace(
            environment={**env, "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(rustflags)},
            configuration=(),
            cli_configuration={},
            rustflags=rustflags,
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
    baseline_env = {"CARGO_TARGET_DIR": str(tmp_path / "target")}

    def admission(env, rust_cpu=RustCpuPolicy.BASELINE):
        return compiler_identity.backend_build_admission(
            tmp_path,
            ("native-backend",)
            if product is HostToolProduct.BACKEND
            else product.required_features,
            "release",
            env,
            rust_cpu=rust_cpu,
            product=product,
        )

    with cache_fingerprints._source_tree_fingerprint_transaction():
        baseline = admission(baseline_env)
        assert admission(baseline_env) is baseline
        # The same environment under the native policy is another compiler:
        # one operation must never reuse the baseline admission for it.
        native = admission(baseline_env, RustCpuPolicy.NATIVE)
        assert native is not baseline
        assert len(seen) == 2
    remaps = (
        f"--remap-path-prefix={tmp_path}=/molt/source",
        f"--remap-path-prefix={tmp_path / 'target'}=/molt/build",
    )
    assert baseline.plan.rustflags == ("-Copt-level=2", *remaps)
    assert native.plan.rustflags == (
        "-Copt-level=2",
        "-C",
        "target-cpu=native",
        *remaps,
    )
    assert native.fingerprint != baseline.fingerprint
    # The policy reaches the plan as a value; the Cargo environment the plan
    # starts from carries no Rust CPU flag of its own.
    assert all("target-cpu" not in "".join(env.values()) for env in seen)
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
    # Transitive build-script/rustc reads belong to the observed receipt, not
    # a guessed family of semantic-plan environment names.
    for variable in (
        "MOLT_TEST_BACKEND_INPUT",
        "SOURCE_DATE_EPOCH",
        "MACOSX_DEPLOYMENT_TARGET",
        "CRATE_CC_NO_DEFAULTS",
        "LLVM_SYS_999_FAKE_SETTING",
        "CARGO_BUILD_UNKNOWN_SETTING",
    ):
        assert (
            admission({**baseline_env, variable: "first"}).fingerprint
            == baseline.fingerprint
        )
        assert (
            admission({**baseline_env, variable: "second"}).fingerprint
            == baseline.fingerprint
        )
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
    tmp_path, monkeypatch, mutate_during_admission, runtime_fixture_root
):
    from molt import llvm_toolchain, llvm_link_plan
    from molt.cli import cargo_execution, runtime_cargo_plan
    from tests.runtime_build_identity_helper import runtime_cargo_plan as fixture_plan

    contract = tmp_path / "config/llvm_toolchain_arches.toml"
    contract.parent.mkdir()
    contract.write_bytes(
        (
            Path(__file__).resolve().parents[2] / "config/llvm_toolchain_arches.toml"
        ).read_bytes()
    )
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
        (prefix / "bin/clang++", b"driver"),
        (prefix / "lib/libc++.dylib", b"C++ ABI"),
        (prefix / "lib/libstdc++.so", b"C++ ABI"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda root, **kw: SimpleNamespace(
            prefix=prefix,
            llvm_config=llvm_config,
            version="22.1.8",
            host_target="x86_64-unknown-linux-gnu",
            link_closure=("lib/libLLVM.a",),
            tool_versions=(
                llvm_toolchain.LlvmToolVersionFact(
                    "clang++",
                    "bin/clang++",
                    "22.1.8",
                    6,
                    hashlib.sha256(b"driver").hexdigest(),
                ),
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
        llvm_toolchain, "llvm_sdk_probe_environment", ProcessEnvironment
    )
    monkeypatch.setattr(
        llvm_link_plan, "_driver_library_directories", lambda *_args: (prefix / "lib",)
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "project_llvm_toolchain_environment",
        lambda root, verification, **kw: dict(kw["environ"]),
    )
    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))
    original_projection = runtime_cargo_plan.RuntimeCargoPlan.toolchain_identity

    def project_tools(plan):
        if mutate_during_admission:
            header.write_bytes(b"/* changed after resource capture */")
        return original_projection(plan)

    monkeypatch.setattr(
        runtime_cargo_plan.RuntimeCargoPlan, "toolchain_identity", project_tools
    )

    def resolve(root, **kwargs):
        environment = kwargs.pop("environment_transform")(kwargs.pop("env"))
        return fixture_plan(
            root, fixture_root=runtime_fixture_root, env=environment, **kwargs
        )

    monkeypatch.setattr(
        runtime_cargo_plan,
        "resolve_runtime_cargo_plan",
        resolve,
    )
    env = {
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "LLVM_SYS_221_PREFIX": str(prefix),
    }

    def admission():
        return compiler_identity.backend_build_admission(
            tmp_path,
            ("llvm", "native-backend"),
            "release",
            env,
            rust_cpu=RustCpuPolicy.BASELINE,
        )

    def rewrite(path, content):
        metadata = path.stat()
        path.write_bytes(content)
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))

    if mutate_during_admission:
        with cache_fingerprints._source_tree_fingerprint_transaction():
            with pytest.raises(
                compiler_identity.CompilerIdentityError, match="inputs changed"
            ):
                admission()
            transaction = cache_fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
            assert transaction is not None and transaction.compiler_plans == {}
        return

    before = admission()
    rewrite(library, b"LLVM-two")
    after_library = admission()
    assert after_library.fingerprint != before.fingerprint
    assert after_library.plan.build_directory != before.plan.build_directory
    rewrite(header, b"/* core, revised */")
    after_header = admission()
    assert after_header.fingerprint != after_library.fingerprint
    assert after_header.plan.build_directory != after_library.plan.build_directory
    # The selected C++ ABI library is observed/captured even though Rust keeps
    # its intentional OS ABI selector. It must invalidate reuse like SDK bytes.
    cpp = (
        prefix
        / "lib"
        / ("libc++.dylib" if sys.platform == "darwin" else "libstdc++.so")
    )
    rewrite(cpp, b"new ABI")
    with pytest.raises(compiler_identity.CompilerIdentityError):
        after_header.verify()
    after_abi = admission()
    assert after_abi.fingerprint != after_header.fingerprint
    assert after_abi.plan.build_directory != after_header.plan.build_directory
    # Only the consumed content set is a build input; unrelated SDK files are not.
    rewrite(unrelated, b"# lldb, revised")
    after_unrelated = admission()
    assert after_unrelated.fingerprint == after_abi.fingerprint
    assert after_unrelated.plan.build_directory == after_abi.plan.build_directory


def test_compiler_cargo_projection_never_admits_runtime_build_python(
    tmp_path, monkeypatch
):
    from molt.cli import cargo_execution, runtime_cargo_plan
    from molt.python_runtime_admission import BuildPythonAdmission

    monkeypatch.setattr(cargo_execution, "_cargo_build_env", lambda env: dict(env))
    monkeypatch.setattr(
        BuildPythonAdmission,
        "capture",
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
            verify=cargo_resource_custody.CargoResourceCustody.capture(()).verify,
            link_resources=SimpleNamespace(content_identity=lambda: {}),
        ),
    )

    def key(python):
        return compiler_identity.backend_build_admission(
            tmp_path,
            ("native-backend",),
            "release",
            {"CARGO_TARGET_DIR": str(tmp_path / "target"), "MOLT_BUILD_PYTHON": python},
            rust_cpu=RustCpuPolicy.BASELINE,
        ).fingerprint

    assert key("missing-runtime-python-a") == key("missing-runtime-python-b")


def test_runtime_captures_shared_cargo_projection_without_python(monkeypatch):
    from molt.cli import runtime_build_identity
    from molt.python_runtime_admission import BuildPythonAdmission

    calls = []
    cargo_content = {"tools": {"rustc": "content"}, "cargo_configuration": "config"}

    def cargo():
        calls.append("cargo-custody")
        return cargo_content

    cargo_custody = cargo_resource_custody.CargoResourceCustody.capture(())

    def verify():
        calls.append("cargo-verify")
        cargo_custody.verify()

    monkeypatch.setattr(
        BuildPythonAdmission,
        "capture",
        lambda *_a, **_k: pytest.fail("Rust capture admitted Python"),
    )
    result = runtime_build_identity._capture_plan_toolchain(
        SimpleNamespace(
            environment={"MOLT_BUILD_PYTHON": "missing-generator"},
            toolchain_identity=cargo,
            verify=verify,
        )
    )
    assert calls == ["cargo-verify", "cargo-custody"]
    assert result is cargo_content
    assert "build_python" not in result["tools"]

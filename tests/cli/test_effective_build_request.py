from __future__ import annotations

import json

from molt.compilation_target import admit_target

from molt.compilation_target import target_name

from types import SimpleNamespace
from pathlib import Path

import pytest

from molt.cli import build_inputs, wrapper_build
from molt.cli import build_request_resolution as selection
from molt.cli.entrypoint_parser import _build_entrypoint_parser
from molt.target_python import _parse_target_python_version


@pytest.mark.parametrize("guest", ["3.12", "3.13", "3.14"])
@pytest.mark.parametrize("mode", ["file", "module"])
def test_compare_default_oracle_follows_guest_while_frontend_stays_315(
    tmp_path, monkeypatch, capsys, guest, mode
):
    from molt.cli import script_commands as scripts
    from molt import python_environment_location as location
    from molt.python_interpreter import PythonInterpreter

    source = tmp_path / "program.py"
    source.write_text("print('match')\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        f'[project]\nname="comparison"\nversion="1"\nrequires-python=">={guest}"\n',
        encoding="utf-8",
    )
    output = tmp_path / "compiled"
    output.write_bytes(b"not executed by this command-boundary fixture")
    frontend = "independent-compiler-host"
    monkeypatch.setattr(
        scripts,
        "sys",
        SimpleNamespace(
            **{
                **vars(scripts.sys),
                "executable": frontend,
                "version_info": (3, 15, 1),
                "prefix": str(tmp_path / "frontend-image"),
            }
        ),
    )
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(
        location, "_active_environment_prefix", lambda: tmp_path / "frontend-image"
    )
    ambient = {
        "PYTHONPYCACHEPREFIX": "foreign-parent-cache",
        "PYTHONDONTWRITEBYTECODE": "0",
        "PAYLOAD_ENV": "preserved",
    }
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: dict(ambient))
    calls = []
    prefixes = []

    def resolve(target, **kwargs):
        assert target.short == guest
        prefix = kwargs["verifier"].prefix
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)
        calls.append("identity")
        return PythonInterpreter(
            ("matching-oracle",), "matching-oracle", guest + ".1", "CPython"
        )

    def execute(command, **kwargs):
        calls.append(list(command))
        assert kwargs["memory_guard_prefix"] == "MOLT_DIFF"
        if command[0] == "matching-oracle":
            assert prefixes[-1].is_dir() and not list(prefixes[-1].iterdir())
            assert command[1:4] == [
                "-B",
                "-X",
                f"pycache_prefix={prefixes[-1] / 'bytecode'}",
            ]
            expected_payload = [str(source)] if mode == "file" else ["-m", "program"]
            assert command[4:] == [*expected_payload, "--payload-flag"]
            assert dict(kwargs["env"]) == ambient
            return SimpleNamespace(
                returncode=0,
                duration_s=0.01,
                stdout="match\n",
                stderr="",
                descendants_closed=True,
            )
        if command[0] == frontend:
            assert not prefixes[-1].exists()
            assert command[1:8] == [
                "-B",
                "-X",
                f"pycache_prefix={tmp_path / 'frontend-image' / 'bytecode'}",
                "-m",
                "molt.cli",
                "build",
                "--json",
            ]
            return SimpleNamespace(
                returncode=0,
                duration_s=0.01,
                stdout=json.dumps({"output": str(output)}),
                stderr="",
                descendants_closed=True,
            )
        assert command == [str(output), "--payload-flag"]
        return SimpleNamespace(
            returncode=0,
            duration_s=0.01,
            stdout="match\n",
            stderr="",
            descendants_closed=True,
        )

    monkeypatch.setattr(scripts, "resolve_target_python", resolve)
    monkeypatch.setattr(
        scripts,
        "resolve_python_selector",
        lambda *_a, **_k: pytest.fail("default used ambient oracle selector"),
    )
    monkeypatch.setattr(scripts, "_run_command_timed", execute)
    from molt import process_guard

    monkeypatch.setattr(process_guard, "run_completed_command", execute)
    monkeypatch.setattr(scripts, "_resolve_binary_output", lambda *_a: output)
    assert (
        scripts.compare(
            str(source) if mode == "file" else None,
            "program" if mode == "module" else None,
            None,
            ["--payload-flag"],
            json_output=True,
        )
        == 0
    )
    policy = json.loads(capsys.readouterr().out)["data"]["python_startup_policy"]
    assert policy == {
        "command": calls[1][:4],
        "bytecode_namespace": "fresh-empty-read-only",
        "timing_scope": "reference-process-with-empty-bytecode-cache",
    }
    assert calls[0] == "identity"
    assert calls[1][0] == "matching-oracle"
    assert calls[2][0] == frontend
    assert prefixes and all(not p.exists() for p in prefixes)


@pytest.mark.parametrize("reported", ["3.13.17", "3.15.1"])
def test_compare_absolute_oracle_mismatch_refuses_before_either_payload(
    tmp_path, monkeypatch, reported
):
    from molt.cli import script_commands as scripts
    from molt.python_interpreter import PythonInterpreter

    source = tmp_path / "program.py"
    source.write_text(
        "raise AssertionError('payload must not start')\n", encoding="utf-8"
    )
    oracle = str(tmp_path / "explicit-oracle")
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: {})
    prefixes = []

    def select(selector, **kwargs):
        assert selector == oracle
        prefix = kwargs["verifier"].prefix
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)
        return (oracle,)

    def probe(command, **kwargs):
        assert command == (oracle,)
        assert kwargs["verifier"].prefix == prefixes[-1]
        return PythonInterpreter(command, oracle, reported, "CPython")

    monkeypatch.setattr(scripts, "resolve_python_selector", select)
    monkeypatch.setattr(scripts, "probe_python_command", probe)
    monkeypatch.setattr(
        scripts,
        "_run_command_timed",
        lambda *_a, **_k: pytest.fail("mismatched oracle reached payload"),
    )
    assert (
        scripts.compare(
            str(source), None, oracle, [], build_args=["--python-version=3.12"]
        )
        != 0
    )
    assert prefixes and all(not p.exists() for p in prefixes)


def test_compare_refuses_unowned_frontend_prefix_before_either_payload(
    tmp_path, monkeypatch
):
    from molt import python_environment_location as location
    from molt.cli import script_commands as scripts
    from molt.python_identity_common import PythonEnvironmentIdentityError
    from molt.python_interpreter import PythonInterpreter

    source = tmp_path / "never-run.py"
    source.write_text("raise AssertionError('payload started')\n", encoding="utf-8")
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: {})
    prefixes = []

    def resolve(target, **kwargs):
        assert target.short == "3.12"
        prefixes.append(kwargs["verifier"].prefix)
        return PythonInterpreter(
            ("matching-oracle",), "matching-oracle", "3.12.15", "CPython"
        )

    def refuse():
        raise PythonEnvironmentIdentityError("ambiguous frontend prefix ownership")

    monkeypatch.setattr(scripts, "resolve_target_python", resolve)
    monkeypatch.setattr(location, "_active_environment_prefix", refuse)
    monkeypatch.setattr(
        scripts,
        "_run_command_timed",
        lambda *_a, **_k: pytest.fail("unowned frontend reached a workload"),
    )
    assert (
        scripts.compare(
            str(source), None, None, [], build_args=["--python-version=3.12"]
        )
        != 0
    )
    assert len(prefixes) == 1 and not prefixes[0].exists()


@pytest.mark.parametrize(
    "kind,closed",
    [
        ("timeout", True),
        ("timeout", False),
        ("infrastructure", True),
        ("infrastructure", False),
        ("interrupt", True),
        ("interrupt", False),
        ("ordinary", None),
    ],
)
def test_compare_reference_failure_retains_only_unclosed_owned_namespace(
    tmp_path, monkeypatch, kind, closed
):
    import gc
    import subprocess
    import weakref

    from molt import temporary_artifacts
    from molt.cli import script_commands as scripts
    from molt.process_guard import GuardInfrastructureError
    from molt.python_interpreter import PythonInterpreter

    source = tmp_path / "program.py"
    source.write_text("pass\n", encoding="utf-8")
    owners = []
    identities = []
    calls = []

    original_allocation = temporary_artifacts.OwnedTemporaryDirectory

    def allocate(**kwargs):
        owner = original_allocation(dir=tmp_path, **kwargs)
        owners.append(owner)
        identities.append(temporary_artifacts._identity(Path(owner.name)))
        return owner

    receipt = SimpleNamespace(
        descendants_closed=closed,
        child_returncode=None,
        infrastructure_failure=SimpleNamespace(
            phase="drain", details=("independent guard failure",)
        ),
    )
    if kind == "timeout":
        failure = subprocess.TimeoutExpired(["reference-oracle"], 1)
    elif kind == "infrastructure":
        failure = GuardInfrastructureError(["reference-oracle"], receipt)
    elif kind == "interrupt":
        failure = KeyboardInterrupt()
    else:
        failure = RuntimeError("no terminal guard observation")
    if closed is not None:
        failure.guarded_result = receipt

    def execute(command, **kwargs):
        calls.append(command)
        assert command[0] == "reference-oracle", "failure reached compiler build"
        root = Path(owners[0].name)
        (root / "still-used").write_bytes(b"reference namespace stays owned")
        raise failure

    monkeypatch.setattr(temporary_artifacts, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: {})
    monkeypatch.setattr(
        scripts,
        "resolve_target_python",
        lambda *_a, **_k: PythonInterpreter(
            ("reference-oracle",), "reference-oracle", "3.12.15", "CPython"
        ),
    )
    monkeypatch.setattr(scripts, "_run_command_timed", execute)
    from molt import process_guard

    monkeypatch.setattr(process_guard, "run_completed_command", execute)
    with pytest.raises(type(failure)) as caught:
        scripts.compare(
            str(source), None, None, [], build_args=["--python-version=3.12"]
        )
    assert caught.value is failure
    if closed is not None:
        assert caught.value.guarded_result is receipt
    assert len(calls) == 1 and len(owners) == 1
    path = Path(owners[0].name)
    assert path.exists() is (closed is not True)
    if closed is not True:
        assert (path / "still-used").read_bytes() == b"reference namespace stays owned"
        assert any(str(path) in note for note in failure.__notes__)
    owner_ref = weakref.ref(owners[0])
    # The primary traceback owns the compare frame too. Retire that test-owned
    # observation only after asserting exception/receipt identity, then prove
    # the resource survives the actual allocation owner's collection.
    failure.__traceback__ = None
    del caught
    owners.clear()
    gc.collect()
    assert owner_ref() is None
    assert path.exists() is (closed is not True)
    if closed is not True:
        # No real child was launched by this synthetic guard control. Retire
        # using the inode receipt captured while the canonical owner was live.
        temporary_artifacts._cleanup_temporary_directory(path, identities[0])


def test_compare_refuses_unclosed_reference_result_and_retains_namespace(
    tmp_path, monkeypatch
):
    import gc
    import weakref

    from molt import temporary_artifacts
    from molt.cli import script_commands as scripts
    from molt.cli.process_execution import _TimedResult
    from molt.python_interpreter import PythonInterpreter

    source = tmp_path / "program.py"
    source.write_text("pass\n", encoding="utf-8")
    owners = []
    identities = []
    calls = []

    original_allocation = temporary_artifacts.OwnedTemporaryDirectory

    def allocate(**kwargs):
        owner = original_allocation(dir=tmp_path, **kwargs)
        owners.append(owner)
        identities.append(temporary_artifacts._identity(Path(owner.name)))
        return owner

    def execute(command, **kwargs):
        calls.append(command)
        assert command[0] == "reference-oracle", (
            "unclosed result reached compiler build"
        )
        return _TimedResult(0, "apparent success", "", 0.1, descendants_closed=False)

    monkeypatch.setattr(temporary_artifacts, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: {})
    monkeypatch.setattr(
        scripts,
        "resolve_target_python",
        lambda *_a, **_k: PythonInterpreter(
            ("reference-oracle",), "reference-oracle", "3.12.15", "CPython"
        ),
    )
    monkeypatch.setattr(scripts, "_run_command_timed", execute)
    from molt import process_guard

    monkeypatch.setattr(process_guard, "run_completed_command", execute)
    with pytest.raises(RuntimeError, match="process-tree closure") as caught:
        scripts.compare(
            str(source), None, None, [], build_args=["--python-version=3.12"]
        )
    assert len(calls) == 1 and len(owners) == 1
    path = Path(owners[0].name)
    assert path.is_dir() and any(str(path) in note for note in caught.value.__notes__)
    owner_ref = weakref.ref(owners[0])
    caught.value.__traceback__ = None
    del caught
    owners.clear()
    gc.collect()
    assert owner_ref() is None and path.is_dir()
    temporary_artifacts._cleanup_temporary_directory(path, identities[0])


@pytest.mark.parametrize(
    "flags,config,target,backend,python",
    [
        (["--target", "native", "--target", "wasm"], {}, "wasm", "cranelift", None),
        (["--target=wasm", "--target=native"], {}, "native", "cranelift", None),
        (["--target=wasm", "--target", "native"], {}, "native", "cranelift", None),
        (["--target", "native", "--target=wasm"], {}, "wasm", "cranelift", None),
        ([], {"target": "wasm"}, "wasm", "cranelift", None),
        ([], {"target": "llvm"}, "native", "llvm", None),
        (["--target", "llvm"], {}, "native", "llvm", None),
        (
            ["--target", "aarch64-unknown-linux-gnu"],
            {},
            "aarch64-unknown-linux-gnu",
            "cranelift",
            None,
        ),
        (
            ["--python-version", "3.12", "--python-version=3.14"],
            {},
            "native",
            "cranelift",
            "3.14",
        ),
        (
            ["--python-version=3.14", "--python-version", "3.12"],
            {},
            "native",
            "cranelift",
            "3.12",
        ),
        (["--backend=llvm", "--backend", "cranelift"], {}, "native", "cranelift", None),
    ],
)
def test_public_and_wrapper_request_select_same_literal_coordinate(
    monkeypatch, tmp_path, flags, config, target, backend, python
):
    args = _build_entrypoint_parser().parse_args(["build", "main.py", *flags])
    public = selection.resolve_build_request(
        args, build_cfg=config, cfg_capabilities=None, env={}
    )
    reads = []
    monkeypatch.setattr(
        selection,
        "_load_molt_config",
        lambda root: reads.append(root) or {"build": config},
    )
    wrapped = selection.resolve_build_arguments(
        flags, file_path="main.py", module=None, project_root=tmp_path, env={}
    )
    assert reads == [tmp_path]
    assert wrapped == public
    assert (
        target_name(wrapped.target),
        wrapped.codegen_backend,
        wrapped.python_version,
    ) == (
        target,
        backend,
        python,
    )


def test_resolver_uses_only_supplied_namespace_config_and_environment(monkeypatch):
    args = _build_entrypoint_parser().parse_args(
        [
            "build",
            "main.py",
            "--platform=cloudflare",
            "--wasm-profile=auto",
            "--precompile",
            "--stdlib-profile=micro",
            "--stdlib-profile",
            "full",
            "--no-respect-pythonpath",
            "--respect-pythonpath",
            "--lib-path=one",
            "--lib-path",
            "two",
        ]
    )
    monkeypatch.setattr(
        selection,
        "_load_molt_config",
        lambda *_: pytest.fail("pure resolver read config"),
    )
    monkeypatch.setattr("sys.argv", ["irrelevant", "--wasm-profile=pure"])
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", "edge")
    request = selection.resolve_build_request(
        args,
        build_cfg={"lib_paths": ["three"]},
        cfg_capabilities=None,
        env={"MOLT_STDLIB_PROFILE": "server"},
    )
    assert request.stdlib_profile == "full"
    assert request.wasm_profile == "auto"
    assert request.precompile is True
    assert request.respect_pythonpath is True
    assert request.lib_paths == ["one", "two", "three"]
    args = _build_entrypoint_parser().parse_args(
        ["build", "main.py", "--platform=cloudflare"]
    )
    request = selection.resolve_build_request(
        args, build_cfg={}, cfg_capabilities=None, env={"MOLT_STDLIB_PROFILE": "server"}
    )
    assert (request.wasm_profile, request.precompile, request.stdlib_profile) == (
        "pure",
        False,
        "server",
    )


@pytest.mark.parametrize(
    "flags,expected",
    [
        ([], "auto"),
        (["--wasm-profile=auto"], "auto"),
        (["--wasm-profile=full"], "full"),
        (["--wasm-profile=pure"], "pure"),
        (["--platform=cloudflare"], "pure"),
        (["--platform=cloudflare", "--wasm-profile=auto"], "auto"),
    ],
)
def test_wasm_profile_default_and_explicit_deployment_override(flags, expected):
    args = _build_entrypoint_parser().parse_args(
        ["build", "main.py", "--target=wasm", *flags]
    )
    request = selection.resolve_build_request(
        args, build_cfg={}, cfg_capabilities=None, env={}
    )
    assert request.wasm_profile == expected


@pytest.mark.parametrize(
    "flags,profile,opt,split",
    [
        (["--platform=wasi"], "release", "O3", False),
        (["--platform=wasi", "--build-profile=dev"], "dev", None, False),
        (["--profile=browser"], "release", "Oz", True),
    ],
)
def test_deployment_and_build_profiles_remain_distinct(flags, profile, opt, split):
    args = _build_entrypoint_parser().parse_args(["build", "main.py", *flags])
    request = selection.resolve_build_request(
        args, build_cfg={}, cfg_capabilities=None, env={}
    )
    assert (request.profile, request.wasm_opt_level, request.split_runtime) == (
        profile,
        opt,
        split,
    )


@pytest.mark.parametrize(
    "flags,config",
    [
        (["--backend=cranelift"], {"target": "llvm"}),
        (["--profile=dev", "--build-profile=release"], {}),
        (["--profile=browser", "--platform=cloudflare"], {}),
    ],
)
def test_conflicts_reject_before_any_build(flags, config):
    args = _build_entrypoint_parser().parse_args(["build", "main.py", *flags])
    with pytest.raises(ValueError):
        selection.resolve_build_request(
            args, build_cfg=config, cfg_capabilities=None, env={}
        )


def test_entry_roots_receive_canonical_last_flag_and_config(monkeypatch, tmp_path):
    captured = {}
    monkeypatch.setattr(
        selection,
        "_load_molt_config",
        lambda _: {"build": {"lib_paths": ["cfg"], "python_version": "3.14"}},
    )
    monkeypatch.setattr(
        build_inputs, "_resolve_build_entry", lambda **kw: (captured.update(kw), None)
    )
    build_inputs._resolve_wrapper_build_entry(
        file_path="main.py",
        module=None,
        project_root=tmp_path,
        json_output=False,
        command="run",
        build_args=[
            "--no-respect-pythonpath",
            "--respect-pythonpath",
            "--lib-path=a",
            "--lib-path",
            "b",
            "--python-version=3.14",
            "--python-version",
            "3.12",
        ],
        env={},
    )
    assert captured["respect_pythonpath"] is True
    assert captured["lib_paths"] == ["a", "b", "cfg"]
    assert captured["target_python"].short == "3.12"


def test_actual_cache_and_admission_consumers_use_effective_target(
    monkeypatch, tmp_path
):
    source = tmp_path / "main.py"
    source.write_text("pass\n", encoding="utf-8")
    request = selection.resolve_build_arguments(
        ["--target=native", "--target=wasm"],
        file_path=str(source),
        module=None,
        project_root=tmp_path,
        env={},
    )
    entry = SimpleNamespace(
        source_path=source,
        entry_module="main",
        target_python=_parse_target_python_version("3.12"),
        external_module_roots=(),
    )
    observed = []
    monkeypatch.setattr(
        wrapper_build,
        "_resolve_import_admission_policy",
        lambda **kw: (observed.append(kw["target"]), None),
    )
    assert (
        wrapper_build._wrapper_build_dependency_fingerprints(
            resolved_build_entry=entry,
            request=request,
            build_args=[],
            env={},
            project_root=tmp_path,
        )
        is None
    )
    assert [target_name(item) for item in observed] == ["wasm"]
    monkeypatch.setattr(
        wrapper_build,
        "_wrapper_build_dependency_fingerprints",
        lambda **kw: ([], {"image": {}}),
    )
    monkeypatch.setattr(wrapper_build, "_cache_tooling_fingerprint", lambda: "tooling")
    features = []
    cpus = []
    monkeypatch.setattr(
        wrapper_build,
        "_cache_fingerprint",
        lambda **kw: (
            features.append(kw["backend_features"]),
            cpus.append(kw["rust_cpu"]),
            "backend",
        )[-1],
    )
    result = wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=entry,
        request=request,
        build_args=[],
        env={},
        project_root=tmp_path,
    )
    assert result is not None
    assert features == [("wasm-backend",)]
    assert result[0]["request"]["target"] == "wasm"
    # The canonical policy preserves baseline CPU for a non-native target.
    from molt.cli.rust_cpu_policy import RustCpuPolicy

    result = wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=entry,
        request=request,
        build_args=[],
        env={"MOLT_PERF_PROFILE": "native-arch"},
        project_root=tmp_path,
    )
    assert result is not None
    assert cpus == [RustCpuPolicy.BASELINE, RustCpuPolicy.BASELINE]

    # Identical argv/source identity cannot reuse a differently resolved profile.
    from dataclasses import replace

    changed = wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=entry,
        request=replace(request, profile="dev"),
        build_args=[],
        env={"MOLT_PERF_PROFILE": "native-arch"},
        project_root=tmp_path,
    )
    assert changed is not None
    assert changed[1] != result[1]


@pytest.mark.parametrize(
    "flags,configured_target,expected",
    [
        (["--build-arg=--target=native", "--build-arg=--target=wasm"], None, "wasm"),
        (["--build-arg=--target=wasm", "--build-arg=--target=native"], None, "native"),
        ([], "wasm", "wasm"),
    ],
)
def test_run_runtime_route_follows_effective_build_request(
    monkeypatch, tmp_path, flags, configured_target, expected
):
    from molt.cli import entrypoint, script_commands

    source = tmp_path / "main.py"
    source.write_text("pass\n", encoding="utf-8")
    if configured_target:
        (tmp_path / "molt.toml").write_text(
            '[build]\ntarget = "' + configured_target + '"\n', encoding="utf-8"
        )
    seen = []
    monkeypatch.setattr(
        script_commands,
        "run_script",
        lambda *a, **kw: seen.append(("native", a[11])) or 0,
    )
    monkeypatch.setattr(
        script_commands,
        "_run_script_cross",
        lambda *a, **kw: seen.append((target_name(a[0]), a[12])) or 0,
    )
    monkeypatch.setattr("sys.argv", ["molt", "run", *flags, str(source)])
    assert entrypoint.main() == 0
    assert seen == [(expected, "dev")]


def test_wasm_run_checks_effective_flags_not_superseded_occurrences():
    from molt.cli.script_commands import _wasm_run_build_args

    flags = _wasm_run_build_args(
        [
            "--target=native",
            "--target",
            "wasm",
            "--no-linked",
            "--linked",
            "--no-require-linked",
            "--require-linked",
        ]
    )
    parsed = selection.parse_build_arguments(flags)
    assert parsed.target == "wasm" and parsed.linked and parsed.require_linked
    with pytest.raises(ValueError, match="molt run --target wasm"):
        _wasm_run_build_args(["--target=wasm", "--target=native"])


def test_reused_parser_does_not_retain_append_values(tmp_path):
    one = selection.resolve_build_arguments(
        ["--lib-path=one"], file_path="a.py", module=None, project_root=tmp_path, env={}
    )
    two = selection.resolve_build_arguments(
        [], file_path="b.py", module=None, project_root=tmp_path, env={}
    )
    assert one.lib_paths == ["one"]
    assert two.lib_paths is None


@pytest.mark.parametrize(
    "flags,expected",
    [
        (["--profile=dev", "--profile=browser"], False),
        (["--profile=browser", "--profile=release"], True),
        (["--release"], True),
        (["--platform=browser"], False),
        (["--build-profile=dev", "--profile=browser"], True),
    ],
)
def test_wrapper_profile_default_presence_uses_final_parsed_selection(flags, expected):
    from molt.cli.arg_helpers import _build_args_has_profile_flag

    assert _build_args_has_profile_flag(flags) is expected


@pytest.mark.parametrize(
    "flags", [["--unknown-option"], ["--emit=invalid"], ["--target"]]
)
def test_wrapper_parser_errors_remain_with_calling_diagnostic_owner(flags, capsys):
    with pytest.raises(ValueError) as error:
        selection.parse_build_arguments(flags)
    assert str(error.value) != "Invalid build arguments"
    assert capsys.readouterr() == ("", "")


@pytest.mark.parametrize(
    "failure",
    ["invalid", "python", "emit", "selector", "oracle", "build", "build-status"],
)
def test_compare_capability_temporary_custody_and_preflight(
    monkeypatch, tmp_path, failure
):
    from molt.cli import script_commands as scripts

    source = tmp_path / "program.py"
    source.write_text("pass\n", encoding="utf-8")
    created = []
    real_materialize = scripts.materialize_capability_input

    def materialize(value):
        result = real_materialize(value)
        assert result[1] is not None and result[1].exists()
        created.append(result[1])
        return result

    monkeypatch.setattr(scripts, "materialize_capability_input", materialize)
    monkeypatch.setattr(scripts, "_base_env", lambda *a, **kw: {})
    monkeypatch.setattr(scripts, "_find_project_root", lambda p: tmp_path)
    calls = []
    prefixes = []

    def select(*a, **kw):
        calls.append("select")
        prefix = kw["verifier"].prefix
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)
        if failure == "selector":
            raise ValueError("unavailable oracle")
        from molt.python_interpreter import PythonInterpreter

        return PythonInterpreter(
            ("oracle-python",), "oracle-python", "3.12.15", "CPython"
        )

    def execute(argv, **kw):
        kind = "oracle" if argv[0] == "oracle-python" else "build"
        calls.append(kind)
        assert created[0].exists()
        if failure == kind:
            error = RuntimeError(kind)
            error.guarded_result = SimpleNamespace(descendants_closed=True)
            raise error
        return SimpleNamespace(
            returncode=1,
            duration_s=0,
            stdout="",
            stderr="failed",
            descendants_closed=True,
        )

    monkeypatch.setattr(scripts, "resolve_target_python", select)
    monkeypatch.setattr(scripts, "_run_command_timed", execute)
    from molt import process_guard

    monkeypatch.setattr(process_guard, "run_completed_command", execute)
    flags = ["--unknown-option"] if failure == "invalid" else []
    if failure == "emit":
        flags = ["--emit=obj"]
    if failure == "python":
        flags = ["--python-version=3.99"]

    def invoke():
        return scripts.compare(
            str(source),
            None,
            None,
            [],
            capabilities={"allow": []},
            build_args=flags,
            json_output=True,
        )

    if failure in {"oracle", "build"}:
        with pytest.raises(RuntimeError, match=failure):
            invoke()
    else:
        assert invoke() != 0
    assert all(not prefix.exists() for prefix in prefixes)
    assert len(created) == 1
    assert not created[0].exists()
    if failure in {"invalid", "python", "emit"}:
        assert calls == []  # Neither oracle discovery nor workload precedes admission.


@pytest.mark.parametrize(
    "route,failure",
    [
        ("native", "status"),
        ("native", "exception"),
        ("luau", "status"),
        ("luau", "exception"),
        ("luau", "notice"),
    ],
)
def test_run_capability_temporary_custody(monkeypatch, tmp_path, route, failure):
    from molt.cli import script_commands as scripts

    source = tmp_path / "program.py"
    source.write_text("pass\n", encoding="utf-8")
    created = []
    real_materialize = scripts.materialize_capability_input

    def materialize(value):
        result = real_materialize(value)
        assert result[1] is not None and result[1].exists()
        created.append(result[1])
        return result

    monkeypatch.setattr(scripts, "materialize_capability_input", materialize)
    monkeypatch.setattr(scripts, "_base_env", lambda *a, **kw: {})
    monkeypatch.setattr(scripts, "_find_project_root", lambda p: tmp_path)
    monkeypatch.setattr(
        scripts._build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kw: (object(), None),
    )

    def build(**kw):
        assert created[0].exists()
        if failure == "exception":
            raise RuntimeError("build failed")
        return None, 0.0, 1

    def notice(*a):
        if failure == "notice":
            assert created[0].exists()
            raise RuntimeError("notice failed")

    monkeypatch.setattr(scripts, "_run_wrapper_build", build)
    monkeypatch.setattr(scripts, "_notice", notice)
    if route == "native":

        def invoke():
            return scripts.run_script(str(source), None, [], capabilities={"allow": []})
    else:

        def invoke():
            return scripts._run_script_cross(
                admit_target(route), str(source), None, [], capabilities={"allow": []}
            )

    if failure in {"exception", "notice"}:
        with pytest.raises(RuntimeError):
            invoke()
    else:
        assert invoke() == 1
    assert len(created) == 1
    assert not created[0].exists()


@pytest.mark.parametrize("raises", [False, True])
def test_deploy_preserves_caller_owned_capability_file(monkeypatch, tmp_path, raises):
    from molt.cli import script_commands as scripts

    source = tmp_path / "program.py"
    source.write_text("pass\n", encoding="utf-8")
    policy = tmp_path / "capabilities.json"
    policy.write_text('{"allow": []}', encoding="utf-8")
    monkeypatch.setattr(scripts, "_base_env", lambda *a, **kw: {})
    monkeypatch.setattr(scripts, "_find_project_root", lambda p: tmp_path)
    monkeypatch.setattr(
        scripts._build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kw: (object(), None),
    )

    def build(**kw):
        assert policy.read_text(encoding="utf-8") == '{"allow": []}'
        if raises:
            raise RuntimeError("build failed")
        return None, 0.0, 1

    monkeypatch.setattr(scripts, "_run_wrapper_build", build)

    def invoke():
        return scripts._deploy(
            "roblox",
            str(source),
            None,
            None,
            None,
            None,
            None,
            "",
            True,
            ["--capabilities", str(policy)],
            True,
            False,
        )

    if raises:
        with pytest.raises(RuntimeError):
            invoke()
    else:
        assert invoke() == 1
    assert policy.read_text(encoding="utf-8") == '{"allow": []}'


@pytest.mark.parametrize("failure", ["serialization", "write", "interrupt"])
def test_capability_materialization_failure_is_atomic(monkeypatch, tmp_path, failure):
    from molt import capability_policy

    created = []
    real_temporary_file = capability_policy.tempfile.NamedTemporaryFile

    def temporary_file(**kwargs):
        handle = real_temporary_file(dir=tmp_path, **kwargs)
        created.append(handle)
        return handle

    monkeypatch.setattr(
        capability_policy.tempfile, "NamedTemporaryFile", temporary_file
    )
    value = {"allow": [], "unsupported": object()}
    expected = TypeError
    if failure != "serialization":
        expected = OSError if failure == "write" else KeyboardInterrupt

        def failed_write(value, handle, **kwargs):
            handle.write("partial")
            handle.flush()
            assert Path(handle.name).read_text(encoding="utf-8") == "partial"
            raise expected("injected failure")

        monkeypatch.setattr(capability_policy.json, "dump", failed_write)
    with pytest.raises(expected):
        capability_policy.materialize_capability_input(value)
    assert len(created) == 1
    assert created[0].closed
    assert not Path(created[0].name).exists()


@pytest.mark.parametrize("mode", ["default", "current", "numeric", "explicit"])
@pytest.mark.parametrize(
    "kind,closed", [("timeout", False), ("timeout", True), ("spawn", False)]
)
def test_compare_metadata_failure_aborts_before_next_candidate_or_payload(
    tmp_path, monkeypatch, mode, kind, closed
):
    import gc
    import subprocess
    import sys
    import weakref
    from molt import process_guard, temporary_artifacts, toolchain_identity
    from molt.cli import script_commands as scripts
    from molt import python_interpreter

    source = tmp_path / "program.py"
    source.write_text(
        "raise AssertionError('payload must not run')\n", encoding="utf-8"
    )
    executable = tmp_path / "selected-python"
    executable.write_bytes(b"not executed by the synthetic core guard")
    request = f"{sys.version_info.major}.{sys.version_info.minor}"
    if request not in {"3.12", "3.13", "3.14"}:
        request = "3.12"
    selector = {
        "default": None,
        "current": sys.executable,
        "numeric": request,
        "explicit": str(executable),
    }[mode]
    calls = []
    owners = []
    allocation_identities = []
    original_allocation = temporary_artifacts.OwnedTemporaryDirectory

    def allocate(**kwargs):
        allocation = original_allocation(dir=tmp_path, **kwargs)
        owners.append(allocation)
        allocation_identities.append(
            temporary_artifacts._identity(Path(allocation.name))
        )
        return allocation

    receipt = SimpleNamespace(descendants_closed=closed)
    failure = (
        subprocess.TimeoutExpired(["first-metadata"], 1)
        if kind == "timeout"
        else FileNotFoundError("interpreter retired after selection")
    )
    if kind == "timeout":
        failure.guarded_result = receipt

    def run(command, **kwargs):
        calls.append(command)
        assert kwargs["memory_guard_prefix"] == "MOLT_PYTHON_IDENTITY"
        assert "-B" in command and "-I" in command and "-S" in command
        assert Path(owners[0].name).is_dir()
        raise failure

    monkeypatch.setattr(temporary_artifacts, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(scripts, "_find_project_root", lambda *_a: tmp_path)
    monkeypatch.setattr(scripts, "_base_env", lambda *_a, **_k: {})
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **_k: None if name == "uv" else Path(name),
    )
    monkeypatch.setattr(
        python_interpreter,
        "python_command_candidates",
        lambda *_a, **_k: [["first-candidate"], ["must-not-run"]],
    )
    monkeypatch.setattr(process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        scripts,
        "_run_command_timed",
        lambda *_a, **_k: pytest.fail("metadata failure reached payload/build"),
    )
    with pytest.raises(type(failure)) as caught:
        scripts.compare(
            str(source), None, selector, [], build_args=[f"--python-version={request}"]
        )
    assert caught.value is failure
    if kind == "timeout":
        assert caught.value.guarded_result is receipt
    assert len(calls) == 1 and len(owners) == 1
    path = Path(owners[0].name)
    allocation_ref = weakref.ref(owners[0])
    assert path.exists() is (not closed)
    failure.__traceback__ = None
    del caught
    owners.clear()
    gc.collect()
    assert allocation_ref() is None
    assert path.exists() is (not closed)
    if not closed:
        temporary_artifacts._cleanup_temporary_directory(path, allocation_identities[0])

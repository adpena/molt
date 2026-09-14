"""Cargo auxiliary tools share captured executable and generated-output custody."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    cargo_output_environment,
    execution_environment,
    supervisor_custody,
    toolchain_capture,
)


def _tool(root: Path, name: str) -> Path:
    path = root / (name + (".exe" if os.name == "nt" else ""))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(name.encode())
    path.chmod(0o755)
    return path


def _no_processes(monkeypatch):
    def fail(*_args, **_kwargs):
        pytest.fail("selection must not execute the compiler or probe again")

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=fail))


def _outputs(command: list[str]) -> cargo_output_environment.CargoOutputEnvironment:
    return cargo_output_environment.CargoOutputEnvironment.for_invocation(
        command_admission.parse_cargo_invocation(command)
    )


def test_explicit_bindgen_driver_wins_without_discovery(tmp_path, monkeypatch):
    driver = _tool(tmp_path / "chosen tools", "clang")
    ambient = _tool(tmp_path / "ambient", "clang")
    _no_processes(monkeypatch)
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path,
        env={"CLANG_PATH": str(driver), "PATH": str(ambient.parent)},
    )
    assert updates == {"CLANG_PATH": str(driver)}
    assert receipt["source"] == "CLANG_PATH"
    assert receipt["available"] is True
    assert receipt["probes"] == []
    assert receipt["bound_names"] == ["CLANG_PATH"]
    assert "environment" not in receipt


def test_relative_explicit_driver_has_no_shared_build_script_cwd(tmp_path, monkeypatch):
    driver = _tool(tmp_path / "chosen", "clang")
    _no_processes(monkeypatch)
    with pytest.raises(ValueError, match="requires an absolute executable path"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={"CLANG_PATH": str(driver.relative_to(tmp_path))}
        )


def test_bindgen_missing_explicit_driver_does_not_fall_back(tmp_path, monkeypatch):
    ambient = _tool(tmp_path / "ambient", "clang")
    _no_processes(monkeypatch)
    with pytest.raises(ValueError, match="CLANG_PATH must name one executable"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path,
            env={"CLANG_PATH": str(tmp_path / "missing"), "PATH": str(ambient.parent)},
        )


def test_bindgen_selector_uses_exact_llvm_config_bindir(tmp_path, monkeypatch):
    config = _tool(tmp_path / "config", "llvm-config")
    driver = _tool(tmp_path / "llvm bin", "clang")
    ambient = _tool(tmp_path / "ambient", "clang")
    calls = []

    def run(command, **kwargs):
        calls.append(list(command))
        assert kwargs["cwd"] == tmp_path
        assert command == [str(config), "--bindir"]
        return subprocess.CompletedProcess(command, 0, str(driver.parent) + "\n", "")

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path,
        env={"LLVM_CONFIG_PATH": str(config), "PATH": str(ambient.parent)},
    )
    assert updates == {"CLANG_PATH": str(driver), "LLVM_CONFIG_PATH": str(config)}
    assert receipt["source"] == "llvm-config --bindir"
    assert calls == [[str(config), "--bindir"]]
    assert receipt["probes"][0]["stdout"] == str(driver.parent) + "\n"


@pytest.mark.parametrize("returncode,stdout", [(1, ""), (0, ""), (0, "one\ntwo\n")])
def test_bindgen_selector_fails_closed_on_invalid_discovery(
    tmp_path, monkeypatch, returncode, stdout
):
    config = _tool(tmp_path, "llvm-config")
    monkeypatch.setattr(
        toolchain_capture,
        "_COMMANDS",
        SimpleNamespace(
            run=lambda command, **_kwargs: subprocess.CompletedProcess(
                command, returncode, stdout, "diagnostic"
            )
        ),
    )
    with pytest.raises(toolchain_capture.RustLinkCaptureError) as raised:
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={"LLVM_CONFIG_PATH": str(config)}
        )
    assert raised.value.diagnostic["unit"] == "cargo-bindgen"
    assert raised.value.diagnostic["probes"][0]["stderr"] == "diagnostic"


def test_bindgen_path_selects_one_driver_not_installation_inventory(
    tmp_path, monkeypatch
):
    versioned = _tool(tmp_path / "first", "clang-21")
    driver = _tool(tmp_path / "first", "clang")
    other = _tool(tmp_path / "second", "clang")
    _tool(tmp_path / "first", "unrelated-helper")
    _no_processes(monkeypatch)
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path,
        env={"PATH": os.pathsep.join((str(driver.parent), str(other.parent)))},
    )
    assert updates == {"CLANG_PATH": str(driver)}
    assert receipt["source"] == "PATH"
    driver.unlink()
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path,
        env={"PATH": os.pathsep.join((str(versioned.parent), str(other.parent)))},
    )
    assert updates == {"CLANG_PATH": str(versioned)}


def test_bindgen_capability_is_optional_when_unavailable(tmp_path, monkeypatch):
    _no_processes(monkeypatch)
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={}
    )
    assert updates == {}
    assert receipt["available"] is False


def test_ordinary_driver_does_not_enumerate_its_directory(tmp_path, monkeypatch):
    driver = _tool(tmp_path, "clang")
    _no_processes(monkeypatch)

    def forbidden(*_args, **_kwargs):
        pytest.fail(
            "an existing ordinary entrypoint does not require directory listing"
        )

    monkeypatch.setattr(Path, "glob", forbidden)
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"PATH": str(tmp_path)}
    )
    assert updates["CLANG_PATH"] == str(driver)
    assert receipt["search_diagnostics"] == []


def test_inaccessible_implicit_directory_records_signal_and_reaches_later_driver(
    tmp_path, monkeypatch
):
    blocked = tmp_path / "blocked"
    blocked.mkdir()
    driver = _tool(tmp_path / "usable", "clang")
    _no_processes(monkeypatch)
    original = Path.glob

    def glob(path, pattern):
        if path == blocked:
            raise PermissionError(13, "fixture denied directory enumeration", str(path))
        return original(path, pattern)

    monkeypatch.setattr(Path, "glob", glob)
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"PATH": os.pathsep.join((str(blocked), str(driver.parent)))}
    )
    assert updates["CLANG_PATH"] == str(driver)
    assert len(receipt["search_diagnostics"]) == 1
    assert receipt["search_diagnostics"][0]["directory"] == str(blocked)
    assert receipt["search_diagnostics"][0]["exception_type"] == "PermissionError"


def test_failed_implicit_selector_is_retained_without_requiring_unused_capability(
    tmp_path, monkeypatch
):
    config = _tool(tmp_path, "llvm-config")
    monkeypatch.setattr(
        toolchain_capture,
        "_COMMANDS",
        SimpleNamespace(
            run=lambda command, **_kwargs: subprocess.CompletedProcess(
                command, 1, "", "no SDK"
            )
        ),
    )
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"PATH": str(tmp_path)}
    )
    assert set(updates) == {"LLVM_CONFIG_PATH"}
    assert Path(updates["LLVM_CONFIG_PATH"]) == config
    assert receipt["available"] is False
    assert receipt["probes"][0]["stderr"] == "no SDK"


def test_usable_driver_does_not_probe_unused_macos_selector(tmp_path, monkeypatch):
    driver = _tool(tmp_path, "clang")
    _tool(tmp_path, "xcrun")
    monkeypatch.setattr(toolchain_capture.sys, "platform", "darwin")
    _no_processes(monkeypatch)
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"PATH": str(tmp_path)}
    )
    assert updates == {"CLANG_PATH": str(driver)}


def test_bindgen_inputs_survive_filter_and_selected_driver_reaches_supervisor(
    tmp_path, monkeypatch
):
    driver = _tool(tmp_path / "llvm", "clang")
    unrelated = _tool(tmp_path / "llvm", "other")
    prefixed = _tool(tmp_path / "llvm", "wasm32-wasip1-clang")
    cargo = _tool(tmp_path, "cargo")
    _no_processes(monkeypatch)
    inherited = {
        "PATH": str(driver.parent),
        "LIBCLANG_PATH": str(driver.parent),
        "LIBCLANG_STATIC_PATH": str(driver.parent),
        "BINDGEN_EXTRA_CLANG_ARGS": "-DPROJECT=1",
        "BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip1": "--target=wasm32-wasip1 --sysroot=/sdk",
    }
    env, contract = execution_environment._deterministic_execution_environment(
        inherited, override_names=[]
    )
    assert env == inherited
    envelope = command_admission.envelope_for_command(["cargo", "build"])
    env, contract = execution_environment._bind_cargo_build_tool_environment(
        envelope, env, contract, cwd=tmp_path
    )
    assert env["CLANG_PATH"] == str(driver)
    assert (
        env["BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip1"]
        == inherited["BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip1"]
    )
    assert contract["passed_names"] == sorted(env, key=str.casefold)
    identities = execution_environment._execution_environment_executable_identities(
        env, cwd=tmp_path
    )
    assert set(identities) == {"CLANG_PATH"}
    _, fixed = supervisor_custody._supervisor_fixed_images({}, identities, [str(cargo)])
    selected = [row for row in fixed if row["role"] == "env:CLANG_PATH"]
    assert len(selected) == 1
    assert selected[0]["path"] == str(driver)
    assert selected[0]["sha256"] == command_identity._hash_file(driver)
    assert str(unrelated) not in {row["path"] for row in fixed}
    assert str(prefixed) not in {row["path"] for row in fixed}
    driver.write_bytes(b"changed driver")
    after = execution_environment._execution_environment_executable_identities(
        env, cwd=tmp_path
    )
    assert identities != after


@pytest.mark.skipif(
    os.name != "nt", reason="Windows environment names are case-insensitive"
)
@pytest.mark.parametrize("hook,tool", [("CLANG_PATH", "clang"), ("RUSTDOC", "rustdoc")])
def test_bindgen_binding_normalizes_environment_case_without_duplicate_keys(
    tmp_path, monkeypatch, hook, tool
):
    driver = _tool(tmp_path, tool)
    _no_processes(monkeypatch)
    env, contract = execution_environment._deterministic_execution_environment(
        {hook.lower(): str(driver)}, override_names=[hook.lower()]
    )
    env, contract = execution_environment._bind_cargo_build_tool_environment(
        command_admission.envelope_for_command(["cargo", "build"]),
        env,
        contract,
        cwd=tmp_path,
    )
    assert env == {hook: str(driver)}
    assert contract["override_names"] == [hook]


@pytest.mark.parametrize(
    "command", [["cargo", "--help"], ["cargo", "test", "--help"], ["git", "status"]]
)
def test_non_build_command_does_not_select_bindgen(command, tmp_path, monkeypatch):
    def fail(**_kwargs):
        pytest.fail("leaf/non-Cargo command does not select a build capability")

    monkeypatch.setattr(toolchain_capture, "select_cargo_build_tool_environment", fail)
    env, contract = execution_environment._bind_cargo_build_tool_environment(
        command_admission.envelope_for_command(command),
        {},
        {"passed_names": [], "override_names": []},
        cwd=tmp_path,
    )
    assert env == {}
    assert "build_tool_selection" not in contract


@pytest.mark.parametrize(
    "key",
    [
        "CLANG_PATH",
        "LLVM_CONFIG_PATH",
        "RUSTFMT",
        "RUSTDOC",
        "CARGO_BUILD_RUSTDOC",
        "PATH",
        "CARGO_TARGET_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
    ],
)
@pytest.mark.parametrize("inline", [False, True])
def test_config_owned_bindgen_driver_requires_actual_cargo_context(
    tmp_path, monkeypatch, key, inline
):
    config = tmp_path / "selected-cargo.toml"
    value = f'env.{key} = {{ value = "selected-driver", force = true }}'
    config.write_text(value, encoding="utf-8")
    monkeypatch.setattr(
        command_identity,
        "_tool_configuration_identities",
        lambda *_args, **_kwargs: [] if inline else [{"path": str(config)}],
    )
    command = ["cargo", "--config", value if inline else str(config), "build"]
    with pytest.raises(ValueError, match=f"defines env.{key}") as raised:
        execution_environment._require_cargo_build_tool_environment_context(
            command,
            outputs=_outputs(command),
            cwd=tmp_path,
            env={key: "existing-driver"},
        )
    assert "Cargo-owned build-script environment" in str(raised.value)
    assert ("Cargo --config entry 1" if inline else str(config)) in str(raised.value)
    assert "selected-driver" not in str(raised.value)


def test_config_default_driver_is_not_silently_replaced(tmp_path, monkeypatch):
    config = tmp_path / "selected-cargo.toml"
    config.write_text('env.CLANG_PATH = "configured-driver"', encoding="utf-8")
    monkeypatch.setattr(
        command_identity,
        "_tool_configuration_identities",
        lambda *_args, **_kwargs: [{"path": str(config)}],
    )
    with pytest.raises(ValueError, match="defines env.CLANG_PATH"):
        execution_environment._require_cargo_build_tool_environment_context(
            ["cargo", "build"],
            outputs=_outputs(["cargo", "build"]),
            cwd=tmp_path,
            env={},
        )
    # A non-forced default does not supersede an already explicit environment.
    execution_environment._require_cargo_build_tool_environment_context(
        ["cargo", "build"],
        outputs=_outputs(["cargo", "build"]),
        cwd=tmp_path,
        env={"CLANG_PATH": "explicit-driver"},
    )


@pytest.mark.parametrize("inline", [False, True])
@pytest.mark.parametrize("inherited", [False, True])
def test_config_output_default_requires_known_effective_environment(
    tmp_path, monkeypatch, inline, inherited
):
    config = tmp_path / "cargo.toml"
    definition = 'env.CARGO_TARGET_DIR = "outside"'
    config.write_text(definition, encoding="utf-8")
    monkeypatch.setattr(
        command_identity,
        "_tool_configuration_identities",
        lambda *_args, **_kwargs: [] if inline else [{"path": str(config)}],
    )
    command = ["cargo", "--config", definition if inline else str(config), "test"]
    env = {"CARGO_TARGET_DIR": str(tmp_path / "selected")} if inherited else {}
    if inherited:
        execution_environment._require_cargo_build_tool_environment_context(
            command, outputs=_outputs(command), cwd=tmp_path, env=env
        )
    else:
        with pytest.raises(ValueError, match="defines env.CARGO_TARGET_DIR"):
            execution_environment._require_cargo_build_tool_environment_context(
                command, outputs=_outputs(command), cwd=tmp_path, env=env
            )


def test_unrelated_cargo_environment_does_not_require_driver_context(
    tmp_path, monkeypatch
):
    config = tmp_path / "selected-cargo.toml"
    config.write_text('env.PROJECT_MODE = "fixture"', encoding="utf-8")
    monkeypatch.setattr(
        command_identity,
        "_tool_configuration_identities",
        lambda *_args, **_kwargs: [{"path": str(config)}],
    )
    execution_environment._require_cargo_build_tool_environment_context(
        ["cargo", "build"], outputs=_outputs(["cargo", "build"]), cwd=tmp_path, env={}
    )


def test_ignored_config_toml_is_not_mistaken_for_selected_cargo_context(
    tmp_path, monkeypatch
):
    home = tmp_path / "cargo-home"
    home.mkdir()
    config = home / "config"
    config.write_text('env.PROJECT_MODE = "fixture"', encoding="utf-8")
    ignored = home / "config.toml"
    ignored.write_text('env.CLANG_PATH = "ignored-driver"', encoding="utf-8")
    execution_environment._require_cargo_build_tool_environment_context(
        ["cargo", "build"],
        outputs=_outputs(["cargo", "build"]),
        cwd=tmp_path,
        env={"CARGO_HOME": str(home)},
    )
    with pytest.raises(ValueError, match="defines env.CLANG_PATH"):
        execution_environment._require_cargo_build_tool_environment_context(
            ["cargo", "--config", str(ignored), "build"],
            outputs=_outputs(["cargo", "--config", str(ignored), "build"]),
            cwd=tmp_path,
            env={"CARGO_HOME": str(home)},
        )


def test_explicit_clang_still_binds_independent_library_discovery(
    tmp_path, monkeypatch
):
    driver = _tool(tmp_path / "driver", "clang")
    config = _tool(tmp_path / "library", "llvm-config")
    _no_processes(monkeypatch)
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"CLANG_PATH": str(driver), "PATH": str(config.parent)}
    )
    assert set(updates) == {"CLANG_PATH", "LLVM_CONFIG_PATH"}
    assert Path(updates["CLANG_PATH"]) == driver
    assert Path(updates["LLVM_CONFIG_PATH"]) == config


def test_relative_llvm_config_cannot_borrow_invocation_cwd(tmp_path, monkeypatch):
    config = _tool(tmp_path / "library", "llvm-config")
    _no_processes(monkeypatch)
    with pytest.raises(ValueError, match="requires an absolute executable path"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={"LLVM_CONFIG_PATH": str(config.relative_to(tmp_path))}
        )


@pytest.mark.parametrize(
    "key",
    [
        "CLANG_PATH",
        "LLVM_CONFIG_PATH",
        "RUSTFMT",
        "RUSTDOC",
        "CARGO_BUILD_RUSTDOC",
        "CC",
        "AR",
    ],
)
def test_environment_image_projection_keeps_selection_and_resolved_paths(tmp_path, key):
    selected = _tool(tmp_path / "selected", "compiler")
    resolved = _tool(tmp_path / "resolved", "compiler")
    root = _tool(tmp_path, "cargo")
    # Exercise projection of the shared executable identity. No second capture
    # or driver-specific process authority is needed for a resolved image.
    identity = command_identity._executable_identity(selected)
    identity.update(resolved_path=str(resolved), symlinked=True)
    _, images = supervisor_custody._supervisor_fixed_images(
        {}, {key: {"executable": identity}}, [str(root)]
    )
    assert {row["path"] for row in images if row["role"] == f"env:{key}"} == {
        str(selected),
        str(resolved),
    }


def test_default_cargo_home_is_guarded(tmp_path):
    profile = tmp_path / "profile"
    config = profile / ".cargo" / "config.toml"
    config.parent.mkdir(parents=True)
    config.write_text(
        'env.CLANG_PATH = { value = "configured-driver", force = true }',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="defines env.CLANG_PATH"):
        execution_environment._require_cargo_build_tool_environment_context(
            ["cargo", "build"],
            outputs=_outputs(["cargo", "build"]),
            cwd=tmp_path,
            env={"USERPROFILE" if os.name == "nt" else "HOME": str(profile)},
        )


def test_bindgen_formatter_is_an_optional_shared_executable_input(
    tmp_path, monkeypatch
):
    formatter = _tool(tmp_path, "rustfmt")
    _no_processes(monkeypatch)
    env, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"RUSTFMT": str(formatter)}
    )
    assert env == {"RUSTFMT": str(formatter)}
    assert receipt["formatter_available"] is True
    identities = execution_environment._execution_environment_executable_identities(
        env, cwd=tmp_path
    )
    assert set(identities) == {"RUSTFMT"}
    assert identities["RUSTFMT"]["executable"]["sha256"] == command_identity._hash_file(
        formatter
    )
    formatter.write_bytes(b"mutated formatter")
    assert (
        execution_environment._execution_environment_executable_identities(
            env, cwd=tmp_path
        )
        != identities
    )


def test_bindgen_rustup_formatter_is_bound_to_actual_selected_binary(
    tmp_path, monkeypatch
):
    from molt import rust_toolchain

    formatter = _tool(tmp_path / "proxy", "rustfmt")
    rustup = _tool(formatter.parent, "rustup")
    formatter.write_bytes(rustup.read_bytes())
    content = _tool(tmp_path / "selected", "rustfmt")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        assert kwargs["cwd"] == tmp_path
        return subprocess.CompletedProcess(command, 0, str(content) + "\n", "")

    monkeypatch.setattr(rust_toolchain.process_guard, "run_completed_command", run)
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"RUSTFMT": str(formatter)}
    )
    assert updates["RUSTFMT"] == str(content)
    assert calls == [[str(rustup), "which", "rustfmt"]]


def test_relative_bindgen_formatter_does_not_adopt_invocation_cwd(
    tmp_path, monkeypatch
):
    formatter = _tool(tmp_path, "rustfmt")
    _no_processes(monkeypatch)
    with pytest.raises(ValueError, match="requires an absolute executable path"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={"RUSTFMT": formatter.name}
        )


@pytest.mark.parametrize("explicit", [False, True])
def test_missing_rustup_formatter_component_preserves_optional_vs_explicit_policy(
    tmp_path, monkeypatch, explicit
):
    from molt import rust_toolchain

    formatter = _tool(tmp_path / "proxy", "rustfmt")
    rustup = _tool(formatter.parent, "rustup")
    formatter.write_bytes(rustup.read_bytes())
    env = {"RUSTFMT": str(formatter)} if explicit else {"PATH": str(formatter.parent)}

    def unavailable(command, **kwargs):
        assert kwargs["cwd"] == tmp_path
        assert kwargs["env"] == env
        return subprocess.CompletedProcess(command, 1, "", "component not installed\n")

    monkeypatch.setattr(
        rust_toolchain.process_guard, "run_completed_command", unavailable
    )
    if explicit:
        with pytest.raises(rust_toolchain.RustupProxyUnavailable) as raised:
            toolchain_capture.select_cargo_build_tool_environment(cwd=tmp_path, env=env)
        diagnostic = raised.value.diagnostic
    else:
        updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env=env
        )
        assert updates == {}
        assert receipt["formatter_available"] is False
        (diagnostic,) = receipt["probes"]
    assert diagnostic["argv"] == [str(rustup), "which", "rustfmt"]
    assert diagnostic["returncode"] == 1
    assert diagnostic["stderr"] == "component not installed\n"


@pytest.mark.parametrize("lexical_rustup", [False, True])
def test_rustup_formatter_alias_resolves_through_content_directory(
    tmp_path, monkeypatch, lexical_rustup
):
    from molt import rust_toolchain

    rustup = _tool(tmp_path / "real", "rustup")
    alias = tmp_path / "alias" / ("rustfmt.exe" if os.name == "nt" else "rustfmt")
    alias.parent.mkdir()
    try:
        alias.symlink_to(rustup)
    except OSError as exc:
        if os.name == "nt" and exc.winerror == 1314:
            pytest.skip("Windows symlink creation privilege unavailable")
        raise
    if lexical_rustup:
        _tool(alias.parent, "rustup").write_bytes(b"unrelated custom binary")
    actual = _tool(tmp_path / "selected", "rustfmt")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, str(actual) + "\n", "")

    monkeypatch.setattr(rust_toolchain.process_guard, "run_completed_command", run)
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={"RUSTFMT": str(alias)}
    )
    assert Path(updates["RUSTFMT"]) == actual
    assert calls == [[str(rustup), "which", "rustfmt"]]


@pytest.mark.parametrize("stdout", ["", "relative-rustfmt", "one\ntwo\n"])
def test_optional_formatter_does_not_hide_malformed_successful_selector(
    tmp_path, monkeypatch, stdout
):
    from molt import rust_toolchain

    rustup = _tool(tmp_path, "rustup")
    _tool(tmp_path, "rustfmt").write_bytes(rustup.read_bytes())
    monkeypatch.setattr(
        rust_toolchain.process_guard,
        "run_completed_command",
        lambda command, **kwargs: subprocess.CompletedProcess(command, 0, stdout, ""),
    )
    with pytest.raises(ValueError, match="one absolute path"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={"PATH": str(tmp_path)}
        )


def test_cargo_selector_owns_environment_before_all_build_tool_capture(
    tmp_path, monkeypatch
):
    calls = []

    def capture(*, cwd, env, rustdoc_required):
        calls.append(dict(env))
        assert rustdoc_required is True
        return {}, {"bound_names": []}

    monkeypatch.setattr(
        toolchain_capture, "select_cargo_build_tool_environment", capture
    )
    env, contract = execution_environment._deterministic_execution_environment(
        {"RUSTUP_TOOLCHAIN": "older-context"}, override_names=[]
    )
    env, contract = execution_environment._bind_cargo_build_tool_environment(
        command_admission.envelope_for_command(["cargo", "+selected-context", "test"]),
        env,
        contract,
        cwd=tmp_path,
    )
    assert calls[0]["RUSTUP_TOOLCHAIN"] == "selected-context"
    assert env["RUSTUP_TOOLCHAIN"] == "selected-context"
    assert "RUSTUP_TOOLCHAIN" in contract["passed_names"]


@pytest.mark.parametrize("hook", ["RUSTDOC", "CARGO_BUILD_RUSTDOC"])
def test_cargo_documentation_tool_reaches_exact_image_custody(
    tmp_path, monkeypatch, hook
):
    documenter = _tool(tmp_path / "selected toolchain", "rustdoc")
    unrelated = _tool(documenter.parent, "unrelated-helper")
    cargo = _tool(tmp_path, "cargo")
    _no_processes(monkeypatch)
    env, contract = execution_environment._deterministic_execution_environment(
        {hook: str(documenter)}, override_names=[hook]
    )
    env, contract = execution_environment._bind_cargo_build_tool_environment(
        command_admission.envelope_for_command(["cargo", "test"]),
        env,
        contract,
        cwd=tmp_path,
    )
    assert env["RUSTDOC"] == str(documenter)
    assert contract["build_tool_selection"]["rustdoc_required"] is True
    assert contract["build_tool_selection"]["rustdoc_available"] is True
    assert set(contract["passed_names"]) == set(env)
    identities = execution_environment._execution_environment_executable_identities(
        env, cwd=tmp_path
    )
    assert identities["RUSTDOC"]["executable"]["sha256"] == command_identity._hash_file(
        documenter
    )
    _, images = supervisor_custody._supervisor_fixed_images(
        {}, identities, [str(cargo)]
    )
    admitted = [row for row in images if row["role"] == "env:RUSTDOC"]
    assert len(admitted) == 1 and admitted[0]["path"] == str(documenter)
    assert str(unrelated) not in {row["path"] for row in images}
    documenter.write_bytes(b"changed documenter")
    assert (
        identities
        != execution_environment._execution_environment_executable_identities(
            env, cwd=tmp_path
        )
    )


def test_documenter_direct_hook_collapses_shadowed_cargo_hook(tmp_path, monkeypatch):
    selected = _tool(tmp_path / "selected", "rustdoc")
    shadowed = _tool(tmp_path / "shadowed", "rustdoc")
    _no_processes(monkeypatch)
    updates, _ = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path,
        env={"RUSTDOC": str(selected), "CARGO_BUILD_RUSTDOC": str(shadowed)},
        rustdoc_required=True,
    )
    assert updates == {"RUSTDOC": str(selected), "CARGO_BUILD_RUSTDOC": str(selected)}
    identities = execution_environment._execution_environment_executable_identities(
        updates, cwd=tmp_path
    )
    assert {row["executable"]["path"] for row in identities.values()} == {str(selected)}


@pytest.mark.parametrize("explicit", [False, True])
def test_documenter_proxy_binds_selected_component_in_cargo_context(
    tmp_path, monkeypatch, explicit
):
    from molt import rust_toolchain

    proxy = _tool(tmp_path / "proxy", "rustdoc")
    rustup = _tool(proxy.parent, "rustup")
    proxy.write_bytes(rustup.read_bytes())
    actual = _tool(tmp_path / "selected", "rustdoc")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        assert kwargs["env"]["RUSTUP_TOOLCHAIN"] == "selected-toolchain"
        return subprocess.CompletedProcess(command, 0, str(actual) + "\n", "")

    monkeypatch.setattr(rust_toolchain.process_guard, "run_completed_command", run)
    inherited = {"RUSTUP_TOOLCHAIN": "older-context", "PATH": str(proxy.parent)}
    if explicit:
        inherited["RUSTDOC"] = str(proxy)
    env, contract = execution_environment._deterministic_execution_environment(
        inherited, override_names=[]
    )
    env, _ = execution_environment._bind_cargo_build_tool_environment(
        command_admission.envelope_for_command(
            ["cargo", "+selected-toolchain", "test"]
        ),
        env,
        contract,
        cwd=tmp_path,
    )
    assert env["RUSTDOC"] == str(actual)
    assert calls == [[str(rustup), "which", "rustdoc"]]


def test_required_documenter_missing_never_degrades_to_late_custody_failure(
    tmp_path, monkeypatch
):
    _no_processes(monkeypatch)
    with pytest.raises(ValueError, match="requires an available rustdoc"):
        toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env={}, rustdoc_required=True
        )
    updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
        cwd=tmp_path, env={}
    )
    assert updates == {} and receipt["rustdoc_available"] is False


@pytest.mark.parametrize("hook", ["RUSTDOC", "CARGO_BUILD_RUSTDOC"])
def test_documenter_explicit_path_must_be_absolute_and_available(
    tmp_path, monkeypatch, hook
):
    ambient = _tool(tmp_path, "rustdoc")
    _no_processes(monkeypatch)
    for value in [ambient.name, str(tmp_path / "missing-documenter")]:
        with pytest.raises(ValueError, match=hook):
            toolchain_capture.select_cargo_build_tool_environment(
                cwd=tmp_path, env={hook: value, "PATH": str(tmp_path)}
            )


@pytest.mark.parametrize("inline", [False, True])
def test_cargo_owned_documenter_config_requires_explicit_selection(
    tmp_path, monkeypatch, inline
):
    definition = 'build.rustdoc = "configured-documenter"'
    config = tmp_path / "cargo.toml"
    config.write_text(definition, encoding="utf-8")
    monkeypatch.setattr(
        command_identity,
        "_tool_configuration_identities",
        lambda *_args, **_kwargs: [] if inline else [{"path": str(config)}],
    )
    command = ["cargo", "--config", definition if inline else str(config), "test"]
    for env in ({}, {"CARGO_BUILD_RUSTDOC": "potentially-shadowed-documenter"}):
        with pytest.raises(ValueError, match="defines build.rustdoc"):
            execution_environment._require_cargo_build_tool_environment_context(
                command, outputs=_outputs(command), cwd=tmp_path, env=env
            )
    execution_environment._require_cargo_build_tool_environment_context(
        command,
        outputs=_outputs(command),
        cwd=tmp_path,
        env={"RUSTDOC": "explicit-documenter"},
    )


@pytest.mark.parametrize("required", [False, True])
def test_documentation_temporary_executables_stay_in_existing_cargo_root(
    tmp_path, required
):
    target = tmp_path / "leased-generation"
    target.mkdir()
    original = {
        "CARGO_TARGET_DIR": str(target),
        "TEMP": str(tmp_path / "guard-scratch"),
    }
    contract = {
        "passed_names": sorted(original),
        "override_names": [],
        "build_tool_selection": {"rustdoc_required": required},
    }
    command = ["cargo", "test" if required else "build"]
    outputs = _outputs(command)
    env, bound = execution_environment._cargo_output_environment_contract(
        outputs.bind(original, target=target), contract, outputs=outputs, target=target
    )
    roots = supervisor_custody._supervisor_derived_roots(
        descendants="declared-toolchains", env=env
    )
    assert roots == [{"role": "build-output", "path": str(target)}]
    if required:
        for name in ("TMPDIR", "TMP", "TEMP"):
            assert env[name] == str(target)
            assert name in bound["passed_names"] and name in bound["override_names"]
        assert bound["cargo_output_environment"] == outputs.identity()
    else:
        assert env == original
        assert bound["cargo_output_environment"] == outputs.identity()
    assert original["TEMP"] == str(tmp_path / "guard-scratch")


def test_documentation_scratch_cannot_override_native_supervisor_requirements(tmp_path):
    with pytest.raises(ValueError, match="supervisor-owned"):
        execution_environment._cargo_output_environment_contract(
            {},
            {
                "build_tool_selection": {"rustdoc_required": True},
                "supervisor_owned_names": ["TEMP"],
            },
            outputs=_outputs(["cargo", "test"]),
            target=tmp_path,
        )


@pytest.mark.parametrize(
    "explicit,required", [(False, False), (True, False), (False, True)]
)
def test_unavailable_documenter_proxy_never_selects_an_alternate(
    tmp_path, monkeypatch, explicit, required
):
    documenter = _tool(tmp_path, "rustdoc")

    def unavailable(path, *, role, root, env):
        assert path == documenter and role == "rustdoc"
        raise toolchain_capture.RustupProxyUnavailable(
            role, {"stderr": "component unavailable", "unit": role}
        )

    monkeypatch.setattr(toolchain_capture, "resolve_rustup_proxy", unavailable)
    env = {"RUSTDOC": str(documenter)} if explicit else {"PATH": str(tmp_path)}
    if explicit or required:
        with pytest.raises(toolchain_capture.RustupProxyUnavailable):
            toolchain_capture.select_cargo_build_tool_environment(
                cwd=tmp_path, env=env, rustdoc_required=required
            )
    else:
        updates, receipt = toolchain_capture.select_cargo_build_tool_environment(
            cwd=tmp_path, env=env
        )
        assert updates == {} and receipt["rustdoc_available"] is False
        assert receipt["probes"][0]["unit"] == "rustdoc"

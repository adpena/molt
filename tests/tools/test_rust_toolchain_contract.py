from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
import platform

import pytest
from tests.process_guard_common import install_module_view

ROOT = Path(__file__).resolve().parents[2]
CHECK_RUST_TOOLCHAIN = ROOT / "tools" / "check_rust_toolchain.py"


def _load_check_rust_toolchain():
    spec = importlib.util.spec_from_file_location(
        "molt_test_check_rust_toolchain",
        CHECK_RUST_TOOLCHAIN,
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_repository_rust_toolchain_contract_is_canonical() -> None:
    tool = _load_check_rust_toolchain()

    report = tool.check_repository_contract()

    assert report.errors == ()


def test_ci_gate_uses_repo_rust_toolchain_gate_without_compile_slot() -> None:
    spec = importlib.util.spec_from_file_location(
        "molt_test_ci_gate_for_rust_toolchain",
        ROOT / "tools" / "ci_gate.py",
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)

    check = {entry.name: entry for entry in module._build_checks()}["rust-toolchain"]

    assert check.cmd == [
        sys.executable,
        str(module.TOOLS / "check_rust_toolchain.py"),
    ]
    assert check.needs_rust is False
    assert check.needs_cargo is True


def test_cargo_version_probe_normalizes_config_wrapper(monkeypatch) -> None:
    tool = _load_check_rust_toolchain()
    captured: dict[str, object] = {}

    def fake_run(_command: list[str], **kwargs: object):
        captured.update(kwargs)
        return subprocess.CompletedProcess([], 0, "cargo 1.96.1", "")

    monkeypatch.setenv("CARGO_BUILD_RUSTC_WRAPPER", "sccache")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    install_module_view(monkeypatch, "subprocess", subprocess, tool, run=fake_run)

    tool._run(["cargo", "--version"])

    assert captured["env"]["CARGO_INCREMENTAL"] == "0"  # type: ignore[index]
    assert captured["env"]["RUSTUP_AUTO_INSTALL"] == "0"


def test_selected_compiler_meets_workspace_minimum() -> None:
    tool = _load_check_rust_toolchain()
    pinned = tool.RUST_VERSION
    major, minor, _patch = (int(part) for part in pinned.split("."))
    assert not tool.check_compiler_version(
        f"rustc {major}.{minor - 1}.0 (2d76d9bc7 2026-03-09)"
    ).ok
    assert not tool.check_compiler_version(
        f"rustc {pinned}-nightly (abcdef 2026-06-01)"
    ).ok
    assert tool.check_compiler_version(f"rustc {pinned} (31fca3adb 2026-06-26)").ok
    assert tool.check_compiler_version(
        f"rustc {major}.{minor + 2}.0-nightly (c1070d693 2026-09-28)"
    ).ok
    assert not tool.check_compiler_version("garbage").ok


def test_pinned_version_is_the_rust_toolchain_channel() -> None:
    import tomllib

    tool = _load_check_rust_toolchain()
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text("utf-8"))

    assert tool.RUST_VERSION == toolchain["toolchain"]["channel"]


def test_workflows_name_rust_toolchain_roles_not_versions() -> None:
    tool = _load_check_rust_toolchain()
    workflow = Path(".github/workflows/fixture.yml")
    text = (
        "jobs:\n"
        "  a:\n"
        "    steps:\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: pinned\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: sanitizer-nightly\n"
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        f'          rust-toolchain: "{tool.RUST_VERSION}"\n'
        "      - uses: ./.github/actions/setup-project\n"
        "        with:\n"
        "          rust-toolchain: stable\n"
    )

    errors = tool.workflow_rust_toolchain_errors(workflow, text)

    assert [error.split(":", 2)[1] for error in errors] == ["12", "15"]
    assert all("must name a role" in error for error in errors)


def _installation_fixture(tmp_path, monkeypatch):
    tool = _load_check_rust_toolchain()
    (tmp_path / "config").mkdir()
    (tmp_path / "config/rust_nightly_toolchain.txt").write_text(
        "nightly-2026-10-01\n", encoding="utf-8"
    )
    (tmp_path / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel = "1.2.3"\ncomponents = ["clippy", "rustfmt"]\n'
        'targets = ["wasm32-wasip1"]\n',
        encoding="utf-8",
    )
    (tmp_path / "Cargo.toml").write_text(
        '[workspace.package]\nrust-version = "1.0.0"\n', encoding="utf-8"
    )
    monkeypatch.setattr(tool, "ROOT", tmp_path)
    return tool


def test_installation_plan_unions_full_manifest_and_additions(tmp_path, monkeypatch):
    tool = _installation_fixture(tmp_path, monkeypatch)
    plan = tool.installation_plan(
        "pinned", components="rust-src,clippy", targets="aarch64-unknown-linux-gnu"
    )
    assert (plan.channel, plan.components, plan.targets, plan.nightly) == (
        "1.2.3",
        ("clippy", "rust-src", "rustfmt"),
        ("aarch64-unknown-linux-gnu", "wasm32-wasip1"),
        False,
    )
    nightly = tool.installation_plan("sanitizer-nightly", components="miri,rust-src")
    assert (nightly.channel, nightly.components, nightly.targets, nightly.nightly) == (
        "nightly-2026-10-01",
        ("miri", "rust-src"),
        (),
        True,
    )


@pytest.mark.parametrize(
    "fragment",
    [
        'components = "clippy"',
        'components = [""]',
        'components = ["x\\ny"]',
        "components = [42]",
    ],
)
def test_installation_plan_rejects_malformed_manifest_before_commands(
    tmp_path, monkeypatch, fragment
):
    tool = _installation_fixture(tmp_path, monkeypatch)
    (tmp_path / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel = "1.2.3"\ntargets = []\n' + fragment + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        tool, "_run", lambda *_a, **_kw: pytest.fail("malformed plan executed a tool")
    )
    with pytest.raises(ValueError):
        tool.installation_plan("pinned")


@pytest.mark.parametrize("value", ["x,,y", "--force", "x\ny", " x", "x/y"])
def test_installation_plan_rejects_invalid_component_atoms(
    tmp_path, monkeypatch, value
):
    tool = _installation_fixture(tmp_path, monkeypatch)
    with pytest.raises(ValueError):
        tool.installation_plan("pinned", components=value)


def _installed_fixture(tmp_path, monkeypatch, failure="", *, nightly=False):
    tool = _installation_fixture(tmp_path, monkeypatch)
    plan = (
        tool.installation_plan("sanitizer-nightly", components="miri,rust-src")
        if nightly
        else tool.installation_plan("pinned")
    )
    host = "x86_64-unknown-linux-gnu"
    sysroot = tmp_path / "toolchains" / f"{plan.channel}-{host}"
    (sysroot / "bin").mkdir(parents=True)
    binaries = {}
    for name in (
        "rustc",
        "cargo",
        "rustfmt",
        "cargo-clippy",
        "clippy-driver",
        "cargo-miri",
        "miri",
    ):
        path = sysroot / "bin" / name
        path.write_bytes(b"fixture executable")
        binaries[name] = path
    for target in (host, "wasm32-wasip1"):
        library = sysroot / "lib/rustlib" / target / "lib"
        library.mkdir(parents=True)
        for name in ("libcore-fixture.rlib", "libstd-fixture.rlib"):
            (library / name).write_bytes(b"fixture library")
    if nightly:
        for crate in ("core", "std"):
            source = sysroot / "lib/rustlib/src/rust/library" / crate / "src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"independent Rust source")
        if failure == "missing-source":
            source.unlink()
    if failure == "missing-cargo":
        binaries["cargo"].unlink()
    if failure == "empty-cargo":
        binaries["cargo"].write_bytes(b"")
    for driver in ("clippy-driver", "miri"):
        if failure == f"missing-{driver}":
            binaries[driver].unlink()
        if failure == f"empty-{driver}":
            binaries[driver].write_bytes(b"")
        if failure == f"outside-{driver}":
            outside = tmp_path / driver
            outside.write_bytes(b"foreign executable")
            binaries[driver].unlink()
            binaries[driver].symlink_to(outside)
    if failure == "empty-target":
        (sysroot / "lib/rustlib/wasm32-wasip1/lib/libstd-fixture.rlib").unlink()
    calls = []

    def run(command, **_kwargs):
        calls.append(command)
        if command == ["rustc", f"+{plan.channel}", "--version", "--verbose"]:
            release = (
                "1.5.0-nightly"
                if nightly and failure != "stable-nightly"
                else "1.2.4"
                if failure == "wrong-rustc"
                else "1.2.3"
            )
            output = (
                f"rustc {release} (123456789 2026-09-28)\n"
                f"host: {host}\nrelease: {release}\ncommit-hash: {'1' * 40}\n"
            )
        elif command == ["cargo", f"+{plan.channel}", "--version"]:
            output = (
                "cargo 1.5.0-nightly (fixture)"
                if nightly
                else "cargo 1.2.4 (fixture)"
                if failure == "wrong-cargo"
                else "cargo 1.2.3 (fixture)"
            )
        elif command == ["rustc", f"+{plan.channel}", "--print", "sysroot"]:
            output = str(sysroot)
        elif command[:2] == ["rustup", "which"]:
            output = str(binaries[command[-1]])
        elif command[:3] == ["rustup", "run", plan.channel]:
            # Authored from each upstream CLI, not the production probe table.
            invocations = {
                str(binaries["rustfmt"]): (
                    ["--version"],
                    "rustfmt 1.8.0-stable (123456789 2026-09-28)",
                ),
                str(binaries["cargo-clippy"]): (
                    ["--version"],
                    "clippy 0.1.2 (123456789 2026-09-28)",
                ),
                str(binaries["cargo-miri"]): (
                    ["miri", "--version"],
                    "miri 0.1.0 (123456789 2026-09-28)",
                ),
                str(binaries["clippy-driver"]): (
                    ["--rustc", "--version", "--verbose"],
                    None,
                ),
                str(binaries["miri"]): (["--version", "--verbose"], None),
            }
            expected_args, output = invocations[command[3]]
            assert command[4:] == expected_args, "wrong native component dialect"
            name = Path(command[3]).name
            if failure in {"broken-component", f"broken-{name}"}:
                return subprocess.CompletedProcess(command, 1, "", "invalid executable")
            if output is None:
                release = "1.5.0-nightly" if nightly else "1.2.3"
                commit = "2" * 40 if failure == f"wrong-{name}" else "1" * 40
                output = (
                    f"rustc {release} (123456789 2026-09-28)\n"
                    f"host: {host}\nrelease: {release}\ncommit-hash: {commit}\n"
                )
            if failure == "garbage-component-version":
                output = "independent nonempty garbage"
            if failure == "wrong-miri-wrapper" and name == "cargo-miri":
                output = "cargo 1.5.0-nightly (123456789 2026-09-28)"
        elif command[:3] == ["rustup", "component", "list"]:
            components = ["cargo", "rustc", "rust-std", "rustfmt", "clippy"]
            if nightly:
                components.extend(("rust-src", "miri"))
            if failure == "missing-component":
                components.remove("clippy")
            if failure == "missing-miri-component":
                components.remove("miri")
            output = "\n".join(f"{name}-{host}" for name in components)
        elif command[:3] == ["rustup", "target", "list"]:
            output = host if failure == "missing-target" else f"{host}\nwasm32-wasip1"
        else:
            pytest.fail(f"unexpected installer/mutation during validation: {command}")
        return subprocess.CompletedProcess(command, 0, output, "")

    monkeypatch.setattr(tool, "_run", run)
    return tool, plan, calls


def test_complete_installed_toolchain_passes_without_mutation(tmp_path, monkeypatch):
    tool, plan, calls = _installed_fixture(tmp_path, monkeypatch)
    assert tool.check_installed_toolchain(plan).ok
    assert ["cargo", "+1.2.3", "--version"] in calls
    assert any(
        command[:3] == ["rustup", "run", "1.2.3"]
        and Path(command[3]).name == "clippy-driver"
        and command[4:] == ["--rustc", "--version", "--verbose"]
        for command in calls
    )
    assert all(
        "install" not in command and "default" not in command for command in calls
    )


@pytest.mark.parametrize(
    "failure",
    [
        "missing-cargo",
        "empty-cargo",
        "broken-component",
        "garbage-component-version",
        "missing-clippy-driver",
        "empty-clippy-driver",
        "outside-clippy-driver",
        "broken-clippy-driver",
        "wrong-clippy-driver",
        "wrong-rustc",
        "wrong-cargo",
        "missing-component",
        "missing-target",
        "empty-target",
    ],
)
def test_partial_or_wrong_installation_is_not_admitted(tmp_path, monkeypatch, failure):
    tool, plan, _calls = _installed_fixture(tmp_path, monkeypatch, failure)
    report = tool.check_installed_toolchain(plan)
    assert not report.ok, failure
    assert report.errors


def _load_provisioner():
    path = ROOT / ".github/actions/setup-project/provision-rust.py"
    spec = importlib.util.spec_from_file_location("molt_test_rust_provisioner", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("failure", ["", "install", "validation", "default"])
def test_provisioning_is_one_install_then_full_validation_then_default(failure):
    provisioner = _load_provisioner()
    plan = SimpleNamespace(
        channel="1.2.3", components=("clippy", "rustfmt"), targets=("wasm32-wasip1",)
    )
    events = []

    def run(command, **kwargs):
        phase = (
            "install"
            if command[:3] == ["rustup", "toolchain", "install"]
            else "default"
        )
        events.append((phase, command, kwargs))
        return subprocess.CompletedProcess(
            command,
            int(failure == phase),
            "",
            "fixture failure" if failure == phase else "",
        )

    def validate(actual):
        assert actual is plan
        events.append(("validation",))
        return SimpleNamespace(
            ok=failure != "validation",
            errors=("missing Cargo",) if failure == "validation" else (),
        )

    tool = SimpleNamespace(_run=run, check_installed_toolchain=validate)
    if failure:
        with pytest.raises(RuntimeError):
            provisioner.provision(plan, tool)
    else:
        provisioner.provision(plan, tool)
    expected = (
        ["install"]
        if failure == "install"
        else ["install", "validation"]
        if failure == "validation"
        else ["install", "validation", "default"]
    )
    assert [event[0] for event in events] == expected
    assert events[0] == (
        "install",
        [
            "rustup",
            "toolchain",
            "install",
            "1.2.3",
            "--profile",
            "minimal",
            "--component",
            "clippy",
            "--component",
            "rustfmt",
            "--target",
            "wasm32-wasip1",
        ],
        {"timeout": 600.0},
    )


@pytest.mark.parametrize("phase", ["install", "validation", "default"])
def test_provisioning_propagates_exact_exception_without_later_phases(phase):
    provisioner = _load_provisioner()
    plan = SimpleNamespace(channel="1.2.3", components=(), targets=())
    failure = OSError("independent interrupted setup")
    events = []

    def run(command, **_kwargs):
        current = (
            "install"
            if command[:3] == ["rustup", "toolchain", "install"]
            else "default"
        )
        events.append(current)
        if current == phase:
            raise failure
        return subprocess.CompletedProcess(command, 0, "", "")

    def validate(_plan):
        events.append("validation")
        if phase == "validation":
            raise failure
        return SimpleNamespace(ok=True, errors=())

    with pytest.raises(OSError) as caught:
        provisioner.provision(
            plan, SimpleNamespace(_run=run, check_installed_toolchain=validate)
        )
    assert caught.value is failure
    assert (
        events
        == ["install", "validation", "default"][
            : ["install", "validation", "default"].index(phase) + 1
        ]
    )


@pytest.mark.parametrize("failure", ["pin", "implementation", "version", "selected"])
def test_rust_setup_rejects_unadmitted_python_before_repository_import(
    tmp_path, monkeypatch, failure
):
    provisioner = _load_provisioner()
    monkeypatch.setattr(provisioner, "ROOT", tmp_path)
    (tmp_path / ".python-version").write_text(
        "3.12\n" if failure == "pin" else "3.12.15\n", encoding="utf-8"
    )
    install_module_view(
        monkeypatch,
        "platform",
        platform,
        provisioner,
        python_implementation=lambda: (
            "PyPy" if failure == "implementation" else "CPython"
        ),
    )
    monkeypatch.setattr(
        provisioner.platform,
        "python_version",
        lambda: "3.12.14" if failure == "version" else "3.12.15",
    )
    other = tmp_path / "other-interpreter"
    other.write_text("not selected", encoding="utf-8")
    monkeypatch.setenv(
        "UV_PYTHON", str(other) if failure == "selected" else sys.executable
    )
    monkeypatch.setattr(
        provisioner.importlib.util,
        "spec_from_file_location",
        lambda *_args: pytest.fail("repository imported before Python admission"),
    )
    with pytest.raises(RuntimeError):
        provisioner.main()


@pytest.mark.parametrize(
    "failure",
    [
        "",
        "stable-nightly",
        "missing-source",
        "missing-miri",
        "empty-miri",
        "outside-miri",
        "broken-miri",
        "wrong-miri",
        "wrong-miri-wrapper",
        "missing-miri-component",
        "garbage-component-version",
    ],
)
def test_dated_nightly_requires_nightly_compiler_and_complete_source(
    tmp_path, monkeypatch, failure
):
    tool, plan, calls = _installed_fixture(tmp_path, monkeypatch, failure, nightly=True)
    assert plan.channel == "nightly-2026-10-01"
    report = tool.check_installed_toolchain(plan)
    assert report.ok is (failure == ""), report.errors
    if not failure:
        probes = [command for command in calls if command[:2] == ["rustup", "run"]]
        assert {(Path(command[3]).name, tuple(command[4:])) for command in probes} == {
            ("cargo-miri", ("miri", "--version")),
            ("miri", ("--version", "--verbose")),
        }

from __future__ import annotations

from decimal import localcontext
import json
import os
from types import SimpleNamespace
from pathlib import Path

import pytest

from molt.disk_capacity import (
    DEFAULT_MINIMUM_HEADROOM_BYTES,
    DEFAULT_SCRATCH_BUDGET_BYTES,
    DISK_GUARD_HIGH_WATER_ENV,
    SCRATCH_BUDGET_ENV,
    DiskCapacityError,
    minimum_headroom_bytes,
    require_build_capacity,
    require_scratch_capacity,
    scratch_budget_bytes,
)


def test_minimum_headroom_default_and_fractional_override_rounds_up() -> None:
    assert minimum_headroom_bytes({}) == 25 * 1024**3
    assert DEFAULT_MINIMUM_HEADROOM_BYTES == 25 * 1024**3
    assert minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "0.0000000001"}) == 1


def test_minimum_headroom_is_independent_of_decimal_context() -> None:
    with localcontext() as context:
        context.prec = 1
        assert (
            minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "25.0000000001"})
            == DEFAULT_MINIMUM_HEADROOM_BYTES + 1
        )


def test_extreme_positive_fraction_has_one_byte_minimum_without_large_power() -> None:
    assert minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "1e-1000000"}) == 1


@pytest.mark.parametrize(
    "configured",
    [
        "",
        " ",
        "not-a-number",
        "0",
        "-1",
        "nan",
        "NaN",
        "inf",
        "+Infinity",
        "1e999999",
        "\u0660.\u0660",
        "\uff11",
        "1e\u0662",
    ],
)
def test_malformed_nonpositive_or_nonfinite_threshold_rejects(
    configured: str,
) -> None:
    with pytest.raises(DiskCapacityError) as caught:
        minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: configured})

    error = caught.value
    assert error.diagnostic["status"] == "invalid-configuration"
    assert error.diagnostic["configured_value"] == configured
    assert "positive finite" in str(error)
    assert "diagnostic=" in str(error)


def test_capacity_equal_to_threshold_is_admitted(tmp_path: Path) -> None:
    output = tmp_path / "not-created" / "target"
    required = minimum_headroom_bytes({})

    receipt = require_build_capacity(
        [output],
        env={},
        measure_free_bytes=lambda path: required,
    )

    assert not output.exists()
    assert receipt.required_bytes == required
    assert receipt.probes[0].requested_path == output.resolve()
    assert receipt.probes[0].measured_path == tmp_path.resolve()
    assert receipt.probes[0].free_bytes == required
    assert receipt.as_dict()["status"] == "admitted"


def test_capacity_above_threshold_is_admitted(tmp_path: Path) -> None:
    required = minimum_headroom_bytes({})

    receipt = require_build_capacity(
        [tmp_path / "target"],
        env={},
        measure_free_bytes=lambda path: required + 1,
    )

    assert receipt.probes[0].free_bytes == required + 1


def test_one_byte_below_threshold_rejects_with_serialized_diagnostic(
    tmp_path: Path,
) -> None:
    required = minimum_headroom_bytes({})

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [tmp_path / "target"],
            env={},
            measure_free_bytes=lambda path: required - 1,
        )

    error = caught.value
    assert issubclass(DiskCapacityError, ValueError)
    assert error.diagnostic["status"] == "rejected"
    assert error.diagnostic["probes"][0]["free_bytes"] == required - 1
    serialized = str(error).split("diagnostic=", 1)[1]
    assert json.loads(serialized) == error.diagnostic
    assert "Reclaim verified inactive artifacts" in str(error)
    assert "explicitly permitted build output root" in str(error)


def test_multiple_roots_are_deterministic_and_one_low_rejects(
    tmp_path: Path,
) -> None:
    first = tmp_path / "a-root"
    second = tmp_path / "z-root"
    first.mkdir()
    second.mkdir()
    required = minimum_headroom_bytes({})
    free_by_path = {
        first.resolve(): required - 1,
        second.resolve(): required + 1,
    }

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [second / "out", first / "out"],
            env={},
            measure_free_bytes=lambda path: free_by_path[path],
        )

    probes = caught.value.diagnostic["probes"]
    assert [probe["requested_path"] for probe in probes] == [
        str((first / "out").resolve()),
        str((second / "out").resolve()),
    ]
    assert [probe["free_bytes"] for probe in probes] == [
        required - 1,
        required + 1,
    ]


def test_measurement_failure_rejects_and_records_existing_ancestor(
    tmp_path: Path,
) -> None:
    output = tmp_path / "missing" / "nested" / "target"

    def fail_measurement(path: Path) -> int:
        raise OSError("volume is unavailable")

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [output],
            env={},
            measure_free_bytes=fail_measurement,
        )

    probe = caught.value.diagnostic["probes"][0]
    assert probe["requested_path"] == str(output.resolve())
    assert probe["measured_path"] == str(tmp_path.resolve())
    assert probe["free_bytes"] is None
    assert "volume is unavailable" in probe["error"]
    assert not output.exists()


def test_unmeasurable_ancestor_rejects_without_falling_back(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = (tmp_path / "blocked" / "target").resolve()
    original_stat = Path.stat
    measured: list[Path] = []

    def denied_stat(path: Path, *args: object, **kwargs: object):
        if path == output:
            raise PermissionError("ancestor access denied")
        return original_stat(path, *args, **kwargs)

    monkeypatch.setattr(Path, "stat", denied_stat)

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [output],
            env={},
            measure_free_bytes=lambda path: measured.append(path) or 10**15,
        )

    probe = caught.value.diagnostic["probes"][0]
    assert probe["measured_path"] is None
    assert "ancestor access denied" in probe["error"]
    assert measured == []
    monkeypatch.undo()
    assert not output.exists()


def test_repeated_resolved_output_is_measured_once(tmp_path: Path) -> None:
    output = tmp_path / "target"
    observed: list[Path] = []

    receipt = require_build_capacity(
        [output, tmp_path / "." / "target"],
        env={},
        measure_free_bytes=lambda path: observed.append(path) or 10**15,
    )

    assert len(receipt.probes) == 1
    assert observed == [tmp_path.resolve()]
    assert not output.exists()


def test_invalid_configuration_fails_before_filesystem_measurement(
    tmp_path: Path,
) -> None:
    observed: list[Path] = []

    with pytest.raises(DiskCapacityError):
        require_build_capacity(
            [tmp_path / "target"],
            env={DISK_GUARD_HIGH_WATER_ENV: "0"},
            measure_free_bytes=lambda path: observed.append(path) or 10**15,
        )

    assert observed == []
    assert not (tmp_path / "target").exists()


def test_no_output_paths_rejects() -> None:
    with pytest.raises(
        DiskCapacityError, match="build capacity admission requires at least one path"
    ):
        require_build_capacity([], env={}, measure_free_bytes=lambda path: 10**15)


def test_cargo_admission_measures_the_roots_the_cargo_build_writes(
    tmp_path: Path,
) -> None:
    from molt.cargo_execution_policy import (
        cargo_output_paths,
        require_cargo_build_capacity,
    )

    workspace = tmp_path / "workspace"
    target = tmp_path / "artifacts" / "target"
    build = tmp_path / "artifacts" / "build"
    command = ["cargo", "build", "-p", "molt-ir"]
    env = {"CARGO_TARGET_DIR": str(target), "CARGO_BUILD_BUILD_DIR": str(build)}

    assert cargo_output_paths(command, cwd=workspace, env=env) == (target, build)
    # --target-dir names the same root; `--` ends Cargo's own options.
    assert cargo_output_paths(
        [*command, f"--target-dir={target}", "--", "--target-dir", "x"],
        cwd=workspace,
        env={},
    ) == (target,)
    # Without a declared target Cargo writes below the workspace holding cwd.
    assert cargo_output_paths(command, cwd=workspace, env={}) == (workspace,)
    with pytest.raises(ValueError, match="conflicts"):
        cargo_output_paths(
            [*command, "--target-dir", str(tmp_path / "other")],
            cwd=workspace,
            env=env,
        )

    measured: list[Path] = []
    with pytest.raises(DiskCapacityError):
        require_cargo_build_capacity(
            command,
            cwd=workspace,
            env=env,
            measure_free_bytes=lambda path: (
                measured.append(path) or DEFAULT_MINIMUM_HEADROOM_BYTES - 1
            ),
        )
    assert set(measured) == {tmp_path}  # the nearest existing parent of each root


# Modules whose literal Cargo argv starts no build without admission, and why.
_CARGO_ADMISSION_EXCEPTIONS = {
    "src/molt/cli/compiler_identity.py": "its plan runs through _run_cargo_attempt",
    "src/molt/cli/runtime_native_build.py": "its plan runs through _run_cargo_attempt",
    "src/molt/cli/runtime_wasm_build_spec.py": "its plan runs through _run_cargo_attempt",
    "src/molt/cli/installation_diagnostics.py": "tool names, not an argv",
    "tools/perf_calibration.py": "process names, not an argv",
    "tools/perf_scoreboard.py": "process names, not an argv",
    "tools/proof_queue_pkg/cargo_output_layout.py": "tool names, not an argv",
    "tools/proof_queue_pkg/toolchain_capture.py": "a toolchains declaration, not an argv",
    "tools/release/verify_consumer.py": "tool names, not an argv",
}


def _literal_cargo_compiles(path: Path) -> list[int]:
    import ast

    from molt.cargo_execution_policy import cargo_compiles

    tree = ast.parse(path.read_text(encoding="utf-8"))
    # A literal compared against an argv classifies a command; it starts none.
    compared = {
        id(operand)
        for node in ast.walk(tree)
        if isinstance(node, ast.Compare)
        for operand in (node.left, *node.comparators)
    }
    lines: list[int] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.List, ast.Tuple)) or id(node) in compared:
            continue
        prefix: list[str] = []
        for element in node.elts:
            if isinstance(element, ast.Constant) and isinstance(element.value, str):
                prefix.append(element.value)
            elif isinstance(element, ast.JoinedStr) and prefix:
                prefix.append("+toolchain")  # e.g. f"+{RUST_NIGHTLY}"
            else:
                break
        if prefix[:1] == ["cargo"] and cargo_compiles(prefix):
            lines.append(node.lineno)
    return lines


def test_every_tool_that_spells_a_cargo_compile_admits_build_capacity() -> None:
    # HF-101: tools/rust_ir_verifier.py compiled molt-ir below the build floor.
    # Every module that spells a compiling Cargo argv must admit capacity
    # before Cargo starts, or name why its argv never starts unadmitted.
    root = Path(__file__).resolve().parents[1]
    admission = ("admit_cargo_build(", "require_cargo_build_capacity(")
    missing: dict[str, list[int]] = {}
    spelled: set[str] = set()
    for base in ("tools", "src/molt"):
        for path in sorted((root / base).rglob("*.py")):
            relative = path.relative_to(root).as_posix()
            lines = _literal_cargo_compiles(path)
            if not lines:
                continue
            spelled.add(relative)
            source = path.read_text(encoding="utf-8")
            admits = any(call in source for call in admission) or (
                "require_build_capacity(" in source  # proof-queue Cargo owners
            )
            if not admits and relative not in _CARGO_ADMISSION_EXCEPTIONS:
                missing[relative] = lines
    assert missing == {}
    # An exception for a module that no longer spells a Cargo compile is stale.
    assert set(_CARGO_ADMISSION_EXCEPTIONS) <= spelled
    assert "tools/rust_ir_verifier.py" in spelled


def test_scratch_budget_default_and_override() -> None:
    assert scratch_budget_bytes({}) == DEFAULT_SCRATCH_BUDGET_BYTES == 4 * 1024**3
    assert scratch_budget_bytes({SCRATCH_BUDGET_ENV: "0.5"}) == 512 * 1024**2
    with pytest.raises(DiskCapacityError, match=SCRATCH_BUDGET_ENV):
        scratch_budget_bytes({SCRATCH_BUDGET_ENV: "0"})


def test_scratch_admission_measures_each_scratch_volume_against_the_budget(
    tmp_path: Path,
) -> None:
    ram = tmp_path / "ram"
    disk = tmp_path / "disk"
    ram.mkdir()
    disk.mkdir()
    free = {ram.resolve(): 3 * 1024**3, disk.resolve(): 9 * 1024**3}
    env = {SCRATCH_BUDGET_ENV: "4"}

    receipt = require_scratch_capacity(
        [disk / "gs"], env=env, measure_free_bytes=lambda path: free[path]
    )
    assert receipt.required_bytes == 4 * 1024**3
    assert receipt.probes[0].measured_path == disk.resolve()

    with pytest.raises(DiskCapacityError) as caught:
        require_scratch_capacity(
            [ram / "gs", disk / "gs"], env=env, measure_free_bytes=lambda p: free[p]
        )
    message = str(caught.value)
    assert message.startswith("scratch capacity admission rejected")
    assert str(ram.resolve() / "gs") in message
    assert str(disk.resolve() / "gs") not in message.split(" diagnostic=")[0]
    assert "MOLT_SCRATCH_BUDGET_GB" in message


def test_darwin_ample_raw_capacity_never_queries_estimated_capacity(
    tmp_path, monkeypatch
):
    from molt import disk_capacity

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="darwin"))
    required = DEFAULT_MINIMUM_HEADROOM_BYTES
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda path: required
    )

    def unexpected(path):
        raise AssertionError("ample raw capacity must not consult the native estimate")

    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", unexpected)
    probe = require_build_capacity([tmp_path], env={}).probes[0]
    assert probe.free_bytes == probe.available_bytes == required
    assert probe.important_usage_bytes is None
    assert probe.measurement == "filesystem"


@pytest.mark.parametrize(
    "estimated", [0, DEFAULT_MINIMUM_HEADROOM_BYTES - 1, DEFAULT_MINIMUM_HEADROOM_BYTES]
)
def test_darwin_low_raw_capacity_uses_explicit_native_measurement(
    tmp_path, monkeypatch, estimated
):
    from molt import disk_capacity

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="darwin"))
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 7)
    calls = []
    monkeypatch.setattr(
        disk_capacity,
        "_darwin_important_usage_bytes",
        lambda path: calls.append(path) or estimated,
    )
    if estimated < DEFAULT_MINIMUM_HEADROOM_BYTES:
        with pytest.raises(DiskCapacityError) as caught:
            require_build_capacity([tmp_path / "missing"], env={})
        probe = caught.value.diagnostic["probes"][0]
    else:
        probe = (
            require_build_capacity([tmp_path / "missing"], env={}).probes[0].as_dict()
        )
    assert calls == [tmp_path.resolve()]
    assert probe["free_bytes"] == 7
    assert probe["important_usage_bytes"] == probe["available_bytes"] == estimated
    assert probe["measurement"] == "darwin-important-usage"


@pytest.mark.parametrize(
    "bad", [-1, True, "123", None, 1 << 63, OSError("native failure")]
)
def test_darwin_estimate_failure_retains_raw_evidence_without_fallback(
    tmp_path, monkeypatch, bad
):
    from molt import disk_capacity

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="darwin"))
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 7)

    def native(path):
        if isinstance(bad, Exception):
            raise bad
        return bad

    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", native)
    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity([tmp_path], env={})
    probe = caught.value.diagnostic["probes"][0]
    assert probe["free_bytes"] == 7
    assert probe["available_bytes"] is None
    assert probe["measurement"] == "darwin-important-usage"
    assert probe["error"]


@pytest.mark.parametrize("platform", ["linux", "win32"])
def test_other_platforms_keep_their_filesystem_measurement(
    tmp_path, monkeypatch, platform
):
    from molt import disk_capacity

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform=platform))
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 7)

    def unexpected(path):
        raise AssertionError("Darwin API on another host")

    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", unexpected)
    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity([tmp_path], env={})
    assert caught.value.diagnostic["probes"][0]["measurement"] == "filesystem"


@pytest.mark.parametrize(
    "failure", [None, "api", "url", "copy", "missing", "type", "conversion", "negative"]
)
def test_native_capacity_releases_owned_cf_objects(tmp_path, monkeypatch, failure):
    import ctypes
    from molt import disk_capacity

    # Substitute only this owner's native boundary. Process-global ctypes is
    # also used by independent samplers and must remain intact.
    released = []
    encoded_paths = []

    class Function:
        def __init__(self, callback):
            self.callback = callback

        def __call__(self, *args):
            return self.callback(*args)

    class Foundation:
        def __init__(self):
            self.CFURLCreateFromFileSystemRepresentation = Function(self.create)
            self.CFURLCopyResourcePropertyForKey = Function(self.copy)
            self.CFGetTypeID = Function(lambda value: 8 if failure == "type" else 7)
            self.CFNumberGetTypeID = Function(lambda: 7)
            self.CFNumberGetValue = Function(self.number)
            self.CFErrorGetCode = Function(lambda value: 42)
            self.CFRelease = Function(
                lambda value: released.append(
                    value.value if hasattr(value, "value") else value
                )
            )

        def create(self, allocator, path, length, directory):
            assert allocator is None and directory == 1 and length == len(path)
            encoded_paths.append(path)
            return 0 if failure == "url" else 9 + len(encoded_paths)

        def copy(self, url, key, value, error):
            assert url in {10, 11} and key.value == 70
            value._obj.value = 0 if failure == "missing" else url + 10
            if failure == "copy":
                error._obj.value = 30
                return 0
            return 1

        def number(self, value, kind, result):
            assert value.value in {20, 21} and kind == 4
            result._obj.value = (
                -1 if failure == "negative" else 123 + (value.value - 20) * 333
            )
            return int(failure != "conversion")

    foundation = Foundation()
    loads = []

    def api():
        loads.append("capacity")
        if failure == "api":
            raise OSError("native capacity API is unavailable")
        return foundation, ctypes.c_void_p(70)

    monkeypatch.setattr(disk_capacity, "_darwin_capacity_api", api)
    path = tmp_path / "café storage"
    if failure is None:
        assert disk_capacity._darwin_important_usage_bytes(path) == 123
        assert disk_capacity._darwin_important_usage_bytes(path) == 456
    else:
        with pytest.raises((OSError, ValueError)):
            disk_capacity._darwin_important_usage_bytes(path)
    assert loads == ["capacity"] * (2 if failure is None else 1)
    expected_paths = [] if failure == "api" else [os.fsencode(path)]
    if failure is None:
        expected_paths *= 2
    assert encoded_paths == expected_paths
    expected_releases = [] if failure in {"api", "url"} else [20, 10]
    if failure is None:
        expected_releases += [21, 11]
    if failure == "copy":
        expected_releases = [20, 30, 10]
    elif failure == "missing":
        expected_releases = [10]
    assert released == expected_releases


def test_darwin_capacity_binding_uses_native_abi_and_caches_only_the_api(tmp_path):
    import ctypes
    import sys
    from molt import disk_capacity

    if sys.platform != "darwin":
        pytest.skip("CoreFoundation bindings require a Darwin host")
    binding = disk_capacity._darwin_capacity_api()
    assert disk_capacity._darwin_capacity_api() is binding
    cf, key = binding
    assert key.value
    # CFNumber.h declares CFNumberType as CFIndex (signed long on Darwin).
    assert cf.CFNumberGetValue.argtypes == [
        ctypes.c_void_p,
        ctypes.c_long,
        ctypes.c_void_p,
    ]
    assert cf.CFURLCreateFromFileSystemRepresentation.argtypes == [
        ctypes.c_void_p,
        ctypes.c_char_p,
        ctypes.c_long,
        ctypes.c_ubyte,
    ]
    for _ in range(2):
        value = disk_capacity._darwin_important_usage_bytes(tmp_path)
        assert isinstance(value, int) and value >= 0


@pytest.mark.parametrize(
    "gib, expected",
    [
        (0.0, 0),
        (1.0, 1073741824),
        (20.0, 21474836480),
        (2.0**-31, 1),
        (3.0 * 2.0**-31, 2),
        (float.fromhex("0x1.fffffffffffffp+32"), 9223372036854774784),
    ],
)
def test_float_capacity_budget_rounds_up_within_signed_byte_range(gib, expected):
    from molt.disk_capacity import capacity_bytes_from_gib

    assert capacity_bytes_from_gib(gib) == expected


@pytest.mark.parametrize(
    "gib", [-1.0, float("nan"), float("inf"), -float("inf"), 1e308, 8589934592.0]
)
def test_float_capacity_budget_rejects_nonfinite_negative_and_out_of_range(gib):
    from molt.disk_capacity import capacity_bytes_from_gib

    with pytest.raises(ValueError, match="capacity must be finite and non-negative"):
        capacity_bytes_from_gib(gib)

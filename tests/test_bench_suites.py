from __future__ import annotations

from collections import Counter
from copy import deepcopy
from functools import cache
from pathlib import Path
import tomllib
import io
import zipfile

import pytest

import tools.bench_suites as bench_suites
from tools import bench, bench_individual, bench_wasm, benchmark_applications
from tools import profile as profile_tool


ROOT = Path(__file__).resolve().parents[1]


def test_benchmark_runner_tables_use_canonical_suite() -> None:
    assert tuple(bench.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_individual.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_wasm.BENCHMARKS) == bench_suites.BENCHMARKS
    assert "tests/benchmarks/bench_import_time.py" in bench_individual.BENCHMARKS
    assert "tests/benchmarks/bench_procedural_gen.py" in bench_individual.BENCHMARKS

    assert tuple(bench.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench_wasm.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench_wasm.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench.DYNAMIC_BUILTIN_SLICES) == bench_suites.DYNAMIC_BUILTIN_SLICES


def test_every_benchmark_file_has_exactly_one_primary_owner_or_exclusion() -> None:
    allowed_exclusion_kinds = {
        "self_timed",
        "profile_epoch",
        "diagnostic",
        "external_harness",
    }
    primary_suites = {
        "core": bench_suites.BENCHMARKS,
        "websocket": bench_suites.WS_BENCHMARKS,
        "dynamic_builtin": bench_suites.DYNAMIC_BUILTIN_SLICES,
    }
    primary_paths = [path for suite in primary_suites.values() for path in suite]
    counts = Counter(primary_paths)
    assert all(count == 1 for count in counts.values())
    for suite in primary_suites.values():
        assert len(suite) == len(set(suite))

    discovered = {
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "tests" / "benchmarks").glob("bench_*.py")
    }
    runnable = set(primary_paths)
    excluded = set(bench_suites.EXCLUDED_BENCHMARKS)
    assert runnable.isdisjoint(excluded)
    assert discovered == runnable | excluded

    # Smoke is deliberately an alias/subset, not another ownership class.
    assert set(bench_suites.SMOKE_BENCHMARKS) <= set(bench_suites.BENCHMARKS)
    for path, exclusion in bench_suites.EXCLUDED_BENCHMARKS.items():
        assert (ROOT / path).is_file()
        assert exclusion.kind in allowed_exclusion_kinds
        assert exclusion.reason.strip()


def test_molt_benchmark_args_are_canonicalized_for_all_call_shapes() -> None:
    expected_msgpack = ["--stdlib-profile", "full"]
    assert (
        bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark(
            ROOT / "tests" / "benchmarks" / "bench_parse_msgpack.py"
        )
        == expected_msgpack
    )
    assert bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_sum.py") == []

    first = bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
    first.append("--mutated")
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )


def test_msgpack_full_stdlib_args_reach_wasm_and_profile_tools() -> None:
    expected = ["--stdlib-profile", "full"]
    script = "tests/benchmarks/bench_parse_msgpack.py"

    assert bench.molt_args_for_benchmark(script) == expected
    assert bench_individual.molt_args_for_benchmark(script) == expected
    assert bench_wasm.molt_args_for_benchmark(script) == expected
    assert profile_tool.molt_args_for_benchmark("bench_parse_msgpack.py") == expected
    assert bench_wasm.MOLT_ARGS_BY_BENCH[script] == expected


def test_heap_lifecycle_benchmarks_use_canonical_profile_epochs() -> None:
    expected_labels = {
        "bench_heap_canonical_cache.py": {"canonical_cache_hits"},
        "bench_weakref_native.py": {
            "weakref_constructor_cache_hits",
            "weakref_calls",
            "weakref_sticky_hash_hits",
        },
        "bench_gc_pressure.py": {"gc_pressure_allocation"},
        "bench_object_class_lifecycle.py": {
            "bare_object_lifecycle",
            "user_class_lifecycle",
        },
    }
    for filename, labels in expected_labels.items():
        source = (ROOT / "tests" / "benchmarks" / filename).read_text(encoding="utf-8")
        assert 'load_intrinsic("molt_profile_epoch_reset")' in source
        assert 'load_intrinsic("molt_profile_epoch_dump")' in source
        for label in labels:
            assert f'"{label}"' in source

    lifecycle_source = (
        ROOT / "tests" / "benchmarks" / "bench_object_class_lifecycle.py"
    ).read_text(encoding="utf-8")
    assert "allocate_bare_objects(10_000)" in lifecycle_source
    assert "allocate_plain_instances(10_000)" in lifecycle_source
    assert lifecycle_source.count("del value") == 2


_APPLICATION_CATALOG = """
[artifact.example]
name = "example"
version = "1.0"
url = "https://example.org/example-1.0-py3-none-any.whl"
size = 123
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[benchmark]]
id = "example-app"
[benchmark.execution]
driver = "tests/benchmarks/apps/example.py"
packages = ["example"]
data_archive = "example"
data_member = "example/input.txt"
data_path = "data/input.txt"
data_format = "bytes"
oracle = "tests/benchmarks/apps/oracles/example.txt"
run_args = ["data/input.txt", "1"]
molt_args = ["--capabilities", "fs.read"]
"""


def test_application_catalog_resolves_pins_and_owns_snapshot() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    (app,) = benchmark_applications._decode_catalog(raw)
    raw["artifact"]["example"]["sha256"] = "b" * 64
    raw["benchmark"][0]["execution"]["run_args"].append("changed")
    raw["benchmark"][0]["execution"]["molt_args"].append("changed")
    assert app.name == "example-app"
    assert app.packages == (app.data_archive,)
    assert app.data_archive.name == "example"
    assert app.data_archive.version == "1.0"
    assert app.data_archive.size == 123
    assert app.data_archive.sha256 == "a" * 64
    assert app.run_args == ("data/input.txt", "1")
    assert app.molt_args == ("--capabilities", "fs.read")


def test_application_recipe_allows_explicit_empty_compiler_args() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"]["molt_args"] = []
    (app,) = benchmark_applications._decode_catalog(raw)
    assert app.molt_args == ()


@pytest.mark.parametrize("name", ["sqlglot_pipeline", "html5_document"])
def test_application_compiler_permissions_reach_existing_runners(name) -> None:
    script = f"tests/benchmarks/apps/{name}.py"
    expected = ["--capabilities", "fs.read"]
    for runner in (bench_suites, bench, bench_individual, bench_wasm, profile_tool):
        for path in (script, ROOT / script):
            args = runner.molt_args_for_benchmark(path)
            assert args == expected
            args.append("--mutated")
    assert bench_suites.molt_args_for_benchmark(Path("/elsewhere") / script) == []
    assert bench_suites.molt_args_for_benchmark(f"{name}.py") == []


@pytest.mark.parametrize(
    ("field", "value", "diagnostic"),
    [
        ("packages", "example", "list of strings"),
        ("packages", [1], "list of strings"),
        ("packages", ["example", "example"], "duplicate archive"),
        ("packages", ["absent"], "unknown archive reference"),
        ("data_archive", "absent", "unknown archive reference"),
        ("data_archive", False, "nonempty string"),
        ("run_args", "input.txt", "list of strings"),
        ("run_args", [False], "list of strings"),
        ("run_args", ["bad\x00arg"], "NUL"),
        ("molt_args", "--capabilities fs.read", "list of strings"),
        ("molt_args", [False], "list of strings"),
        ("molt_args", ["bad\x00arg"], "NUL"),
        ("data_format", [], "expected bytes or python-literals"),
        ("driver", "../example.py", "portable relative"),
        ("driver", 1, "portable relative"),
        ("data_member", "C:/input.txt", "portable relative"),
        ("data_path", "data/CON", "portable relative"),
        ("oracle", "/oracle.txt", "portable relative"),
    ],
)
def test_application_recipe_rejects_malformed_fields(field, value, diagnostic) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"][field] = value
    with pytest.raises(ValueError, match=diagnostic):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("value", [None, False, [], "recipe"])
def test_application_recipe_requires_table(value) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"] = value
    with pytest.raises(ValueError, match="expected a table"):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("field", ["run_args", "molt_args", "runner"])
def test_application_recipe_rejects_unknown_or_missing_fields(field) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    recipe = raw["benchmark"][0]["execution"]
    if field != "runner":
        del recipe[field]
    else:
        recipe["runner"] = "another-runner"
    with pytest.raises(ValueError, match="missing fields.*unknown fields"):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize(
    ("field", "value", "diagnostic"),
    [
        ("name", "different", "must match archive reference"),
        ("version", 1, "nonempty string"),
        ("size", True, "positive integer"),
        ("size", "123", "positive integer"),
        ("size", 0, "positive integer"),
        ("sha256", "A" * 64, "lowercase hex"),
        ("sha256", "a" * 63, "lowercase hex"),
        ("url", "http://example.org/archive.whl", "credential-free HTTPS"),
        ("url", "https://example.org/archive.whl?token=x", "literal filename"),
        ("url", 3, "nonempty string"),
    ],
)
def test_application_archive_rejects_malformed_pins(field, value, diagnostic) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["artifact"]["example"][field] = value
    with pytest.raises(ValueError, match=diagnostic):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("driver", ["example.py", "EXAMPLE.py"])
def test_application_catalog_rejects_ambiguous_drivers(driver) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    second = deepcopy(raw["benchmark"][0])
    second["id"] = "second-app"
    second["execution"]["driver"] = "tests/benchmarks/apps/" + driver
    raw["benchmark"].append(second)
    with pytest.raises(ValueError, match="duplicate or ambiguous driver"):
        benchmark_applications._decode_catalog(raw)


def test_application_catalog_rejects_duplicate_ids() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"].append({"id": "example-app"})
    with pytest.raises(ValueError, match="duplicate id"):
        benchmark_applications._decode_catalog(raw)


def test_application_selection_uses_one_snapshot_and_exact_path(
    tmp_path, monkeypatch
) -> None:
    catalog = tmp_path / "registry.toml"
    catalog.write_text(_APPLICATION_CATALOG, encoding="utf-8")
    monkeypatch.setattr(benchmark_applications, "CATALOG", catalog)
    monkeypatch.setattr(benchmark_applications, "ROOT", tmp_path)
    monkeypatch.setattr(
        benchmark_applications,
        "applications",
        cache(benchmark_applications.applications.__wrapped__),
    )
    (app,) = benchmark_applications.applications()
    catalog.write_text("invalid TOML [", encoding="utf-8")
    assert benchmark_applications.application_for_script(tmp_path / app.driver) is app
    assert benchmark_applications.applications() == (app,)

    def unexpected_acquisition(*args, **kwargs):
        pytest.fail("unregistered local workload attempted application preparation")

    monkeypatch.setattr(
        benchmark_applications, "prepare_application", unexpected_acquisition
    )
    assert benchmark_applications.prepare_for_script(tmp_path / "example.py") is None
    assert (
        benchmark_applications.prepare_for_script(tmp_path / "other" / app.driver)
        is None
    )


@pytest.mark.parametrize(
    "payload",
    [
        b"sealed \xc3\xa9 output\n",
        b"raw\xff\n",
        b"raw\xef\xbf\xbd\n",
        b"raw\r\nno-final-newline",
    ],
)
def test_application_oracle_records_exact_reference_bytes(
    tmp_path, monkeypatch, payload
) -> None:
    """A real guarded CPython child produces the oracle; bytes are never replaced.

    Invalid UTF-8 must refuse instead of becoming U+FFFD, which would make the
    oracle accept any undecodable byte; a literal U+FFFD stays exact.
    """
    import sys

    from molt.cli.source_package_seal import (
        SourcePackageInput,
        stage_source_package_seal,
    )

    source = tmp_path / "source"
    driver = source / "drivers" / "emit.py"
    driver.parent.mkdir(parents=True)
    driver.write_text(f"import os\nos.write(1, {payload!r})\n", encoding="utf-8")
    project = source / "pyproject.toml"
    project.write_text(
        '[project]\nname = "oracle-fixture"\nversion = "0"\n', encoding="utf-8"
    )
    seal = stage_source_package_seal(
        tmp_path / "seals",
        [
            SourcePackageInput(driver, "drivers/emit.py", "source"),
            SourcePackageInput(project, "pyproject.toml", "project"),
        ],
    )
    pin = benchmark_applications.ArchivePin("fixture", "0", "https://x/f", 1, "0" * 64)
    app = benchmark_applications.Application(
        name="oracle-fixture",
        driver="drivers/emit.py",
        packages=(),
        data_archive=pin,
        data_member="data",
        data_path="data/input",
        data_format="bytes",
        oracle="oracles/oracle-fixture.txt",
        run_args=(),
        molt_args=(),
    )
    monkeypatch.setattr(benchmark_applications, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_applications, "applications", lambda: (app,))
    monkeypatch.setattr(
        benchmark_applications,
        "prepare_application",
        lambda selected, *, include_oracle: (
            benchmark_applications.PreparedApplication(
                seal, ("src",), "oracle/stdout.txt", ()
            )
            if selected is app and include_oracle is False
            else pytest.fail("oracle production prepared the wrong application")
        ),
    )
    monkeypatch.setattr(
        sys, "argv", ["benchmark_applications", "--write-oracle", app.name]
    )
    target = tmp_path / app.oracle
    if payload == b"raw\xff\n":
        with pytest.raises(ValueError, match="not valid UTF-8"):
            benchmark_applications.main()
        assert not target.exists()
    else:
        assert benchmark_applications.main() == 0
        assert target.read_bytes() == payload


def test_application_sql_fixture_retains_semantic_values_without_execution() -> None:
    fixture = benchmark_applications._sql_fixture(
        b'SQL = "SELECT a FROM t"\nTPCH_SCHEMA = {"t": {"a": "INT"}}\n'
        b'raise RuntimeError("must never execute fixture code")\n'
    )
    assert fixture == {"SQL": "SELECT a FROM t", "TPCH_SCHEMA": {"t": {"a": "INT"}}}


@pytest.mark.parametrize(
    "source",
    [
        b"SQL = 3\nTPCH_SCHEMA = {}",
        b'SQL = "select 1"',
        b'SQL = "select 1"\nSQL = "select 2"\nTPCH_SCHEMA = {}',
        b'SQL = "select 1"\nTPCH_SCHEMA = []',
        b'SQL = "select 1"\nTPCH_SCHEMA = {"t": {"a": 3}}',
    ],
)
def test_application_sql_fixture_rejects_incomplete_or_wrong_types(source) -> None:
    with pytest.raises(ValueError, match="upstream SQL fixture"):
        benchmark_applications._sql_fixture(source)


@pytest.mark.parametrize("missing_notice", [False, True])
def test_application_data_wheel_retains_declared_notices(
    tmp_path, missing_notice
) -> None:
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w") as wheel:
        wheel.writestr("example/input.txt", b"dataset bytes")
        wheel.writestr(
            "example-1.0.dist-info/METADATA",
            "Metadata-Version: 2.4\nName: example\nVersion: 1.0\n"
            "License-File: COPYING\nLicense-File: AUTHORS\n\n",
        )
        wheel.writestr("example-1.0.dist-info/licenses/COPYING", b"License terms")
        if not missing_notice:
            wheel.writestr(
                "example-1.0.dist-info/licenses/AUTHORS", b"Upstream authors"
            )
    with zipfile.ZipFile(archive) as wheel:
        if missing_notice:
            with pytest.raises(ValueError, match="missing declared member.*AUTHORS"):
                benchmark_applications._data_inputs(
                    wheel, "example/input.txt", tmp_path
                )
        else:
            data, inputs = benchmark_applications._data_inputs(
                wheel, "example/input.txt", tmp_path
            )
            assert data == b"dataset bytes"
            retained = {item.relative_path: item.source.read_bytes() for item in inputs}
            prefix = "provenance/data-archive/example-1.0.dist-info/"
            assert retained[prefix + "licenses/COPYING"] == b"License terms"
            assert retained[prefix + "licenses/AUTHORS"] == b"Upstream authors"
            assert prefix + "METADATA" in retained
            assert all(item.role == "provenance" for item in inputs)

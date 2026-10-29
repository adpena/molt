"""Prepare declared application inputs through the existing package seal.

This is acquisition/preparation, not a benchmark runner. Timing, output parity,
classification and publication remain owned by the existing benchmark tools.
"""

from __future__ import annotations

import argparse
import ast
from dataclasses import asdict, dataclass
from functools import cache
from email import policy
from email.parser import BytesParser
import json
import os
from pathlib import Path
import stat
import sys
import tomllib
import zipfile
from typing import TYPE_CHECKING, Literal, TypedDict, cast
from urllib.parse import urlsplit

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = _REPO_ROOT

from molt.portable_paths import (  # noqa: E402
    portable_path_component,
    portable_path_identity,
    portable_relative_path,
)

if TYPE_CHECKING:
    from molt.cli.source_package_seal import SourcePackageInput, SourcePackageSeal


CATALOG = ROOT / "bench/corpus/registry.toml"


@dataclass(frozen=True)
class ArchivePin:
    name: str
    version: str
    url: str
    size: int
    sha256: str


@dataclass(frozen=True)
class Application:
    name: str
    driver: str
    packages: tuple[ArchivePin, ...]
    data_archive: ArchivePin
    data_member: str
    data_path: str
    data_format: Literal["bytes", "python-literals"]
    oracle: str
    run_args: tuple[str, ...]
    molt_args: tuple[str, ...]


@dataclass(frozen=True)
class PreparedApplication:
    seal: SourcePackageSeal
    python_roots: tuple[str, ...]
    oracle: str
    run_args: tuple[str, ...]


def _table(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise ValueError(f"{where}: expected a table")
    return cast(dict[str, object], value)


def _string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{where}: expected a nonempty string")
    return value


def _strings(value: object, where: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError(f"{where}: expected a list of strings")
    if any("\x00" in item for item in value):
        raise ValueError(f"{where}: strings must not contain NUL")
    return tuple(value)


def _fields(table: dict[str, object], expected: set[str], where: str) -> None:
    if set(table) != expected:
        raise ValueError(
            f"{where}: missing fields {sorted(expected - set(table))}; "
            f"unknown fields {sorted(set(table) - expected)}"
        )


def _path(value: object, where: str) -> str:
    try:
        return portable_relative_path(value).as_posix()
    except ValueError as exc:
        raise ValueError(f"{where}: {exc}") from exc


def _archive_pin(value: object, key: str) -> ArchivePin:
    # Use the transfer owner's origin admission before any archive is acquired.
    from molt.tool_releases import ToolReleaseError, _archive_https_host

    where = f"artifact.{key}"
    table = _table(value, where)
    _fields(table, {"name", "version", "url", "size", "sha256"}, where)
    name = _string(table["name"], f"{where}.name")
    portable_path_component(key)
    if name != key:
        raise ValueError(f"{where}.name: must match archive reference {key!r}")
    version = _string(table["version"], f"{where}.version")
    url = _string(table["url"], f"{where}.url")
    try:
        _archive_https_host(url)
        parsed = urlsplit(url)
        filename = portable_relative_path(parsed.path.rsplit("/", 1)[-1])
        if len(filename.parts) != 1 or parsed.query or "%" in filename.name:
            raise ValueError("pinned archive URL must name one literal filename")
    except (ValueError, ToolReleaseError) as exc:
        raise ValueError(f"{where}.url: {exc}") from exc
    size = table["size"]
    if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
        raise ValueError(f"{where}.size: expected a positive integer")
    sha256 = _string(table["sha256"], f"{where}.sha256")
    if len(sha256) != 64 or any(c not in "0123456789abcdef" for c in sha256):
        raise ValueError(f"{where}.sha256: expected 64 lowercase hex digits")
    return ArchivePin(name, version, url, size, sha256)


def _decode_catalog(value: object) -> tuple[Application, ...]:
    catalog = _table(value, "application catalog")
    declarations = _table(catalog.get("artifact"), "artifact")
    archives = {key: _archive_pin(raw, key) for key, raw in declarations.items()}
    if len({portable_path_identity(key) for key in archives}) != len(archives):
        raise ValueError("artifact: ambiguous portable archive names")
    benchmarks = catalog.get("benchmark")
    if not isinstance(benchmarks, list):
        raise ValueError("benchmark: expected a list of tables")
    rows: list[Application] = []
    names: set[str] = set()
    drivers: set[str] = set()
    for index, raw in enumerate(benchmarks):
        row = _table(raw, f"benchmark[{index}]")
        name = _string(row.get("id"), f"benchmark[{index}].id")
        if name in names:
            raise ValueError(f"benchmark: duplicate id {name!r}")
        names.add(name)
        if "execution" not in row:
            continue
        where = f"benchmark[{name}].execution"
        plan = _table(row["execution"], where)
        _fields(
            plan,
            {
                "driver",
                "packages",
                "data_archive",
                "data_member",
                "data_path",
                "data_format",
                "oracle",
                "run_args",
                "molt_args",
            },
            where,
        )
        driver = _path(plan["driver"], f"{where}.driver")
        identity = portable_path_identity(driver)
        if identity in drivers:
            raise ValueError(
                f"{where}.driver: duplicate or ambiguous driver {driver!r}"
            )
        drivers.add(identity)
        packages = _strings(plan["packages"], f"{where}.packages")
        if len(set(packages)) != len(packages):
            raise ValueError(f"{where}.packages: duplicate archive references")
        data_archive = _string(plan["data_archive"], f"{where}.data_archive")
        for reference in (*packages, data_archive):
            if reference not in archives:
                raise ValueError(f"{where}: unknown archive reference {reference!r}")
        data_format = plan["data_format"]
        if data_format != "bytes" and data_format != "python-literals":
            raise ValueError(f"{where}.data_format: expected bytes or python-literals")
        rows.append(
            Application(
                name=name,
                driver=driver,
                packages=tuple(archives[key] for key in packages),
                data_archive=archives[data_archive],
                data_member=_path(plan["data_member"], f"{where}.data_member"),
                data_path=_path(plan["data_path"], f"{where}.data_path"),
                data_format=data_format,
                oracle=_path(plan["oracle"], f"{where}.oracle"),
                run_args=_strings(plan["run_args"], f"{where}.run_args"),
                molt_args=_strings(plan["molt_args"], f"{where}.molt_args"),
            )
        )
    return tuple(rows)


@cache
def applications() -> tuple[Application, ...]:
    # Exactly one immutable configuration snapshot, including resolved pins.
    return _decode_catalog(tomllib.loads(CATALOG.read_text(encoding="utf-8")))


def application_for_script(script: Path) -> Application | None:
    absolute = script.expanduser().absolute()
    return next((app for app in applications() if ROOT / app.driver == absolute), None)


def _archive(pin: ArchivePin, downloads: Path) -> Path:
    from molt.tool_releases import provision_archive

    return provision_archive(
        url=pin.url, size=pin.size, sha256=pin.sha256, downloads=downloads
    )


class SQLFixture(TypedDict):
    SQL: str
    TPCH_SCHEMA: dict[str, dict[str, str]]


def _sql_fixture(data: bytes) -> SQLFixture:
    literals: dict[str, object] = {}
    for node in ast.parse(data).body:
        if (
            isinstance(node, ast.Assign)
            and len(node.targets) == 1
            and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id in {"SQL", "TPCH_SCHEMA"}
        ):
            name = node.targets[0].id
            if name in literals:
                raise ValueError(f"upstream SQL fixture: duplicate {name} assignment")
            literals[name] = ast.literal_eval(node.value)
    _fields(literals, {"SQL", "TPCH_SCHEMA"}, "upstream SQL fixture")
    sql = _string(literals["SQL"], "upstream SQL fixture.SQL")
    raw_schema = _table(literals["TPCH_SCHEMA"], "upstream SQL fixture.TPCH_SCHEMA")
    schema: dict[str, dict[str, str]] = {}
    for table, raw_columns in raw_schema.items():
        columns = _table(raw_columns, f"upstream SQL fixture.TPCH_SCHEMA.{table}")
        schema[table] = {
            name: _string(value, f"upstream SQL fixture.TPCH_SCHEMA.{table}.{name}")
            for name, value in columns.items()
        }
    return {"SQL": sql, "TPCH_SCHEMA": schema}


def _wheel_members(wheel: zipfile.ZipFile) -> dict[str, zipfile.ZipInfo]:
    members: dict[str, zipfile.ZipInfo] = {}
    identities: set[str] = set()
    for member in wheel.infolist():
        if member.is_dir():
            continue
        relative = portable_relative_path(member.filename).as_posix()
        identity = portable_path_identity(relative)
        if identity in identities or stat.S_IFMT(member.external_attr >> 16) not in (
            0,
            stat.S_IFREG,
        ):
            raise ValueError("application wheel has aliased or nonregular members")
        identities.add(identity)
        members[relative] = member
    return members


def _package_inputs(
    archive: Path, pin: ArchivePin, directory: Path
) -> list[SourcePackageInput]:
    from molt.cli.source_package_seal import SourcePackageInput
    from molt.tool_releases import open_pinned_archive

    inputs: list[SourcePackageInput] = []
    total = 0
    with open_pinned_archive(archive, size=pin.size, sha256=pin.sha256) as opened:
        with zipfile.ZipFile(opened.stream) as wheel:
            for relative, member in _wheel_members(wheel).items():
                total += member.file_size
                if total > 64 * 1024 * 1024:
                    raise ValueError(
                        "application wheel exceeds declared preparation bound"
                    )
                target = directory / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(wheel.read(member))
                inputs.append(
                    SourcePackageInput(
                        target,
                        "src/" + relative,
                        "source" if relative.endswith(".py") else "resource",
                    )
                )
    return inputs


def _data_inputs(
    bundle: zipfile.ZipFile, data_member: str, directory: Path
) -> tuple[bytes, list[SourcePackageInput]]:
    """Keep data-wheel metadata and declared notices in the sealed provenance."""
    from molt.cli.source_package_seal import SourcePackageInput

    members = _wheel_members(bundle)
    metadata_paths = [
        name
        for name in members
        if len(Path(name).parts) == 2 and name.endswith(".dist-info/METADATA")
    ]
    if len(metadata_paths) != 1:
        raise ValueError(
            "application data wheel requires exactly one distribution METADATA"
        )
    metadata_path = metadata_paths[0]

    def read(name: str) -> bytes:
        member = members.get(name)
        if member is None:
            raise ValueError(
                f"application data wheel is missing declared member {name!r}"
            )
        if member.file_size > 64 * 1024 * 1024:
            raise ValueError("application data wheel member exceeds preparation bound")
        return bundle.read(member)

    metadata_bytes = read(metadata_path)
    metadata = BytesParser(policy=policy.default).parsebytes(metadata_bytes)
    if metadata.defects:
        raise ValueError("application data wheel has malformed distribution METADATA")
    notices = metadata.get_all("License-File", [])
    if notices and metadata.get("Metadata-Version") != "2.4":
        raise ValueError(
            "application data wheel License-File requires Metadata-Version 2.4"
        )
    dist_info = metadata_path.rsplit("/", 1)[0]
    selected = [metadata_path]
    for notice in notices:
        relative = _path(str(notice), "application data wheel License-File")
        selected.append(f"{dist_info}/licenses/{relative}")
    if len(set(selected)) != len(selected):
        raise ValueError(
            "application data wheel has duplicate License-File declarations"
        )
    inputs: list[SourcePackageInput] = []
    total = 0
    for name in selected:
        content = metadata_bytes if name == metadata_path else read(name)
        total += len(content)
        if total > 64 * 1024 * 1024:
            raise ValueError("application data wheel notices exceed preparation bound")
        target = directory / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        inputs.append(
            SourcePackageInput(target, "provenance/data-archive/" + name, "provenance")
        )
    return read(data_member), inputs


def prepare_application(
    app: Application, *, include_oracle: bool = True
) -> PreparedApplication:
    from molt.cli.source_package_seal import (
        SourcePackageInput,
        stage_source_package_seal,
    )
    from molt.dx import artifact_root
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.tool_releases import open_pinned_archive

    store = artifact_root(ROOT, os.environ) / "bench" / "applications"
    store.mkdir(parents=True, exist_ok=True)
    scratch = store / "preparation"
    scratch.mkdir(exist_ok=True)
    with OwnedTemporaryDirectory(prefix="application-", dir=scratch) as temporary:
        work = Path(temporary)
        inputs = [
            SourcePackageInput(ROOT / app.driver, app.driver, "source"),
            SourcePackageInput(
                ROOT / "tests/benchmarks/apps/pyproject.toml",
                "pyproject.toml",
                "project",
            ),
        ]
        pins = {pin.name: asdict(pin) for pin in (*app.packages, app.data_archive)}
        for pin in app.packages:
            archive = _archive(pin, store / "downloads")
            inputs.extend(_package_inputs(archive, pin, work / pin.name))
        pin = app.data_archive
        archive = _archive(pin, store / "downloads")
        with open_pinned_archive(archive, size=pin.size, sha256=pin.sha256) as opened:
            with zipfile.ZipFile(opened.stream) as bundle:
                data, notices = _data_inputs(
                    bundle, app.data_member, work / "data-archive"
                )
                inputs.extend(notices)
        original = work / "upstream-data"
        original.write_bytes(data)
        inputs.append(
            SourcePackageInput(original, "provenance/upstream-data", "provenance")
        )
        if app.data_format == "python-literals":
            data = (
                json.dumps(_sql_fixture(data), sort_keys=True, separators=(",", ":"))
                + "\n"
            ).encode()
        data_file = work / "data"
        data_file.write_bytes(data)
        inputs.append(SourcePackageInput(data_file, app.data_path, "data"))
        provenance = work / "provenance.json"
        provenance.write_text(
            json.dumps(
                {
                    "application": {
                        **asdict(app),
                        "packages": [pin.name for pin in app.packages],
                        "data_archive": app.data_archive.name,
                    },
                    "archives": pins,
                },
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n",
            encoding="utf-8",
        )
        inputs.append(
            SourcePackageInput(provenance, "provenance/application.json", "provenance")
        )
        if include_oracle:
            expected = ROOT / app.oracle
            if not expected.is_file():
                raise ValueError(
                    f"application oracle is absent; explicitly generate {app.name} before measurement"
                )
            inputs.append(SourcePackageInput(expected, "oracle/stdout.txt", "oracle"))
        seal = stage_source_package_seal(store / "source-packages", inputs)
    return PreparedApplication(seal, ("src",), "oracle/stdout.txt", app.run_args)


def prepare_for_script(script: Path) -> PreparedApplication | None:
    app = application_for_script(script)
    return None if app is None else prepare_application(app)


def main() -> int:
    from molt.process_environment import ProcessEnvironment

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write-oracle", required=True, choices=[app.name for app in applications()]
    )
    args = parser.parse_args()
    app = next(app for app in applications() if app.name == args.write_oracle)
    prepared = prepare_application(app, include_oracle=False)
    env = ProcessEnvironment(os.environ)
    env = env.without(name for name in env if env.rule.key(name).startswith("PYTHON"))
    env = env.updated(
        {
            "PYTHONPATH": str(prepared.seal.payload_root / "src"),
            "PYTHONHASHSEED": "0",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    from tools import harness_memory_guard

    result = harness_memory_guard.guarded_completed_process(
        [
            sys.executable,
            "-P",
            "-S",
            "-B",
            str(prepared.seal.payload_root / app.driver),
            *app.run_args,
        ],
        prefix="MOLT_BENCH_ORACLE",
        cwd=prepared.seal.payload_root,
        env=env,
        capture_output=True,
        text=False,
        encoding="utf-8",
        errors="surrogateescape",
        timeout=60,
        limits=harness_memory_guard.limits_from_env("MOLT_BENCH_ORACLE", env),
    )
    from tools.safe_run import observed_run_outcome

    outcome = observed_run_outcome(result, captured_stderr=result.child_stderr)
    if not outcome.ok or outcome.stderr:
        raise ValueError(f"reference oracle production failed: {result.stderr}")
    from molt.cli.source_package_seal import verify_source_package_seal

    verify_source_package_seal(
        prepared.seal.root, expected_sha256=prepared.seal.seal_sha256
    )
    try:
        # Capture kept undecodable bytes as surrogate escapes; the oracle
        # is UTF-8 text, so they refuse here instead of becoming U+FFFD.
        assert outcome.stdout is not None
        oracle = outcome.stdout.encode("utf-8")
    except UnicodeEncodeError as exc:
        raise ValueError(
            "reference oracle production failed: stdout is not valid UTF-8"
        ) from exc
    target = ROOT / app.oracle
    target.parent.mkdir(parents=True, exist_ok=True)
    from molt.file_publication import atomic_write_bytes

    atomic_write_bytes(target, oracle)
    print(f"Recorded complete semantic output: {app.name} ({len(oracle)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

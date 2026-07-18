#!/usr/bin/env python3
"""Generate static runtime-profile schema declarations from one TOML authority."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

from generator_io import generated_file_matches, write_generated_text

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "runtime" / "runtime_profile_schema.toml"
OUT_RUST = (
    ROOT / "runtime" / "molt-runtime" / "src" / "runtime_profile_schema_generated.rs"
)
OUT_PYTHON = ROOT / "src" / "molt" / "_runtime_profile_schema_generated.py"
OUTPUTS = (OUT_RUST, OUT_PYTHON)

_IDENTIFIER = re.compile(r"^[a-z][a-z0-9_]*$")
_SEMANTICS = {"counter", "gauge"}
_RSS_TARGETS = ("linux", "macos", "windows", "wasm", "fallback")


class SchemaError(ValueError):
    """The checked-in schema authority is internally inconsistent."""


def _require_table(value: object, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise SchemaError(f"{context} must be a table")
    return value


def _require_string(value: object, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise SchemaError(f"{context} must be a non-empty string")
    return value


def _require_string_list(value: object, context: str) -> list[str]:
    if not isinstance(value, list) or not value:
        raise SchemaError(f"{context} must be a non-empty string array")
    result = [_require_string(item, f"{context} item") for item in value]
    if len(result) != len(set(result)):
        raise SchemaError(f"{context} contains duplicates")
    return result


def load_schema(path: Path = SOURCE) -> dict[str, Any]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1:
        raise SchemaError("runtime profile authority schema_version must be 1")
    for envelope in ("process", "epoch"):
        row = _require_table(data.get(envelope), envelope)
        version = row.get("schema_version")
        if not isinstance(version, int) or isinstance(version, bool) or version < 1:
            raise SchemaError(f"{envelope}.schema_version must be a positive integer")
        _require_string(row.get("kind"), f"{envelope}.kind")
        fields = _require_string_list(
            row.get("memory_fields"), f"{envelope}.memory_fields"
        )
        if any(not _IDENTIFIER.fullmatch(field) for field in fields):
            raise SchemaError(
                f"{envelope}.memory_fields contains an invalid identifier"
            )

    rss = _require_table(data.get("rss_sources"), "rss_sources")
    if set(rss) != set(_RSS_TARGETS):
        raise SchemaError(f"rss_sources keys must be exactly {list(_RSS_TARGETS)!r}")
    rss_values = [
        _require_string(rss[target], f"rss_sources.{target}") for target in _RSS_TARGETS
    ]
    if len(rss_values) != len(set(rss_values)):
        raise SchemaError("rss source names must be unique")

    sections = data.get("section")
    if not isinstance(sections, list) or not sections:
        raise SchemaError("section must be a non-empty array of tables")
    section_names: set[str] = set()
    metric_names: set[str] = set()
    for index, raw_section in enumerate(sections):
        section = _require_table(raw_section, f"section[{index}]")
        if set(section) != {"name", "metrics"}:
            raise SchemaError(f"section[{index}] keys must be exactly name and metrics")
        name = _require_string(section["name"], f"section[{index}].name")
        if not _IDENTIFIER.fullmatch(name) or name in section_names:
            raise SchemaError(f"invalid or duplicate section name {name!r}")
        section_names.add(name)
        metrics = section["metrics"]
        if not isinstance(metrics, list) or not metrics:
            raise SchemaError(f"section {name!r} must contain metrics")
        local_names: set[str] = set()
        for metric_index, raw_metric in enumerate(metrics):
            metric = _require_table(raw_metric, f"{name}.metrics[{metric_index}]")
            if set(metric) != {"name", "semantic"}:
                raise SchemaError(f"metric {name}[{metric_index}] has unknown keys")
            metric_name = _require_string(metric["name"], f"{name} metric name")
            semantic = _require_string(
                metric["semantic"], f"{name}.{metric_name}.semantic"
            )
            if not _IDENTIFIER.fullmatch(metric_name):
                raise SchemaError(f"invalid metric name {metric_name!r}")
            if metric_name in local_names or metric_name in metric_names:
                raise SchemaError(f"duplicate metric name {metric_name!r}")
            if semantic not in _SEMANTICS:
                raise SchemaError(f"invalid semantic {semantic!r} for {metric_name!r}")
            local_names.add(metric_name)
            metric_names.add(metric_name)
    return data


def _rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render_rust(schema: dict[str, Any]) -> str:
    process = schema["process"]
    epoch = schema["epoch"]
    rss = schema["rss_sources"]
    sections = schema["section"]
    lines = [
        "// @generated by tools/gen_runtime_profile_schema.py from ",
        "runtime/runtime_profile_schema.toml. DO NOT EDIT.\n\n",
        f"pub(crate) const PROCESS_PROFILE_SCHEMA_VERSION: u64 = {process['schema_version']};\n",
        f"pub(crate) const PROCESS_PROFILE_KIND: &str = {_rust_string(process['kind'])};\n",
        f"pub(crate) const PROFILE_EPOCH_SCHEMA_VERSION: u64 = {epoch['schema_version']};\n",
        f"pub(crate) const PROFILE_EPOCH_KIND: &str = {_rust_string(epoch['kind'])};\n",
    ]
    target_cfg = {
        "linux": '#[cfg(target_os = "linux")]\n',
        "macos": '#[cfg(target_os = "macos")]\n',
        "windows": "#[cfg(windows)]\n",
        "wasm": '#[cfg(target_arch = "wasm32")]\n',
        "fallback": '#[cfg(all(not(target_arch = "wasm32"), not(any(target_os = "linux", target_os = "macos", windows))))]\n',
    }
    for target in _RSS_TARGETS:
        lines.append(
            target_cfg[target]
            + f"pub(crate) const RSS_SOURCE_{target.upper()}: &str = {_rust_string(rss[target])};\n"
        )
    lines.extend(
        [
            "\n#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub(crate) enum RuntimeProfileMetricSemantic { Counter, Gauge }\n\n",
            "#[derive(Clone, Copy, Debug)]\n",
            "pub(crate) struct RuntimeProfileSnapshot {\n",
        ]
    )
    for section in sections:
        for metric in section["metrics"]:
            lines.append(f"    pub(crate) {metric['name']}: u64,\n")
    lines.extend(
        [
            "}\n\n",
            "impl RuntimeProfileSnapshot {\n",
            "    pub(crate) fn into_process_payload(self, memory: serde_json::Value) -> serde_json::Value {\n",
            "        let mut root = serde_json::Map::new();\n",
            '        root.insert("schema_version".to_owned(), serde_json::Value::from(PROCESS_PROFILE_SCHEMA_VERSION));\n',
            '        root.insert("kind".to_owned(), serde_json::Value::from(PROCESS_PROFILE_KIND));\n',
        ]
    )
    for section in sections:
        section_name = section["name"]
        lines.append(
            "        {\n            let mut values = serde_json::Map::new();\n"
        )
        for metric in section["metrics"]:
            name = metric["name"]
            lines.append(
                f"            values.insert({_rust_string(name)}.to_owned(), serde_json::Value::from(self.{name}));\n"
            )
        lines.append(
            f"            root.insert({_rust_string(section_name)}.to_owned(), serde_json::Value::Object(values));\n        }}\n"
        )
    lines.extend(
        [
            '        root.insert("memory".to_owned(), memory);\n',
            "        serde_json::Value::Object(root)\n",
            "    }\n",
            "}\n\n",
            "pub(crate) fn runtime_profile_memory_payload(\n",
            "    source: &'static str,\n    available: bool,\n    current_rss_bytes: Option<u64>,\n    peak_rss_bytes: Option<u64>,\n) -> serde_json::Value {\n",
            "    let mut memory = serde_json::Map::new();\n",
        ]
    )
    memory_values = {
        "source": "source",
        "available": "available",
        "current_rss_bytes": "current_rss_bytes",
        "peak_rss_bytes": "peak_rss_bytes",
    }
    for field in process["memory_fields"]:
        lines.append(
            f"    memory.insert({_rust_string(field)}.to_owned(), serde_json::Value::from({memory_values[field]}));\n"
        )
    lines.extend(
        [
            "    serde_json::Value::Object(memory)\n}\n\n",
            "pub(crate) fn runtime_profile_epoch_memory_payload(\n",
            "    start: serde_json::Value,\n    end: serde_json::Value,\n    current_rss_delta_bytes: Option<i64>,\n) -> serde_json::Value {\n",
            "    let mut memory = serde_json::Map::new();\n",
        ]
    )
    epoch_values = {
        "start": "start",
        "end": "end",
        "current_rss_delta_bytes": "current_rss_delta_bytes",
    }
    for field in epoch["memory_fields"]:
        lines.append(
            f"    memory.insert({_rust_string(field)}.to_owned(), serde_json::Value::from({epoch_values[field]}));\n"
        )
    lines.extend(
        [
            "    serde_json::Value::Object(memory)\n}\n\n",
            "#[inline]\n",
            "pub(crate) fn runtime_profile_metric_semantic(section: &str, metric: &str) -> Option<RuntimeProfileMetricSemantic> {\n",
            "    match (section, metric) {\n",
        ]
    )
    for section in sections:
        for metric in section["metrics"]:
            variant = metric["semantic"].capitalize()
            lines.append(
                f"        ({_rust_string(section['name'])}, {_rust_string(metric['name'])}) => Some(RuntimeProfileMetricSemantic::{variant}),\n"
            )
    lines.extend(["        _ => None,\n    }\n}\n"])
    return "".join(lines)


def _format_rust(source: str) -> str:
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        raise RuntimeError("rustfmt is required to generate runtime-profile Rust")
    completed = subprocess.run(
        [rustfmt, "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"rustfmt failed:\n{completed.stderr}")
    return completed.stdout


def _python_tuple(values: list[str], *, indent: str = "") -> str:
    body = "".join(f'{indent}    "{value}",\n' for value in values)
    return f"(\n{body}{indent})"


def _python_frozenset(values: list[str], *, indent: str = "") -> str:
    return f"frozenset({_python_tuple(values, indent=indent)})"


def render_python(schema: dict[str, Any]) -> str:
    process = schema["process"]
    epoch = schema["epoch"]
    sections = schema["section"]
    rss = schema["rss_sources"]
    lines = [
        "# @generated by tools/gen_runtime_profile_schema.py from\n",
        "# runtime/runtime_profile_schema.toml. DO NOT EDIT.\n",
        "from __future__ import annotations\n\n",
        "from types import MappingProxyType\n",
        "from typing import Mapping\n\n",
        f"PROCESS_PROFILE_SCHEMA_VERSION = {process['schema_version']}\n",
        f'PROCESS_PROFILE_KIND = "{process["kind"]}"\n',
        f"PROFILE_EPOCH_SCHEMA_VERSION = {epoch['schema_version']}\n",
        f'PROFILE_EPOCH_KIND = "{epoch["kind"]}"\n\n',
        "PROCESS_SECTION_ORDER = ",
        _python_tuple([section["name"] for section in sections]),
        "\n",
        "PROCESS_COUNTER_KEYS: Mapping[str, frozenset[str]] = MappingProxyType(\n    {\n",
    ]
    for section in sections:
        names = [metric["name"] for metric in section["metrics"]]
        lines.append(
            f'        "{section["name"]}": '
            f"{_python_frozenset(names, indent='        ')},\n"
        )
    lines.extend(
        [
            "    }\n)\n",
            "PROFILE_GAUGE_KEYS: Mapping[str, frozenset[str]] = MappingProxyType(\n    {\n",
        ]
    )
    for section in sections:
        gauges = [
            metric["name"]
            for metric in section["metrics"]
            if metric["semantic"] == "gauge"
        ]
        if gauges:
            lines.append(
                f'        "{section["name"]}": '
                f"{_python_frozenset(gauges, indent='        ')},\n"
            )
    lines.extend(
        [
            "    }\n)\n",
            "PROFILE_DELTA_KEYS: Mapping[str, frozenset[str]] = MappingProxyType(\n",
            "    {section: keys - PROFILE_GAUGE_KEYS.get(section, frozenset())\n",
            "     for section, keys in PROCESS_COUNTER_KEYS.items()}\n",
            ")\n",
            f"PROCESS_MEMORY_FIELDS = {_python_frozenset(process['memory_fields'])}\n",
            f"EPOCH_MEMORY_FIELDS = {_python_frozenset(epoch['memory_fields'])}\n",
            "PROCESS_RSS_SOURCES = frozenset(\n",
            _python_tuple([rss[target] for target in _RSS_TARGETS], indent="    "),
            "\n)\n",
            "UNAVAILABLE_RSS_SOURCES = frozenset(\n",
            _python_tuple([rss["wasm"], rss["fallback"]], indent="    "),
            "\n)\n",
        ]
    )
    return "".join(lines)


def _format_python(source: str) -> str:
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "ruff",
            "format",
            "-",
            "--stdin-filename",
            str(OUT_PYTHON),
        ],
        cwd=ROOT,
        input=source,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"ruff format failed:\n{completed.stderr}")
    return completed.stdout


def render_all(schema: dict[str, Any]) -> dict[Path, str]:
    return {
        OUT_RUST: _format_rust(render_rust(schema)),
        OUT_PYTHON: _format_python(render_python(schema)),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if outputs are stale"
    )
    args = parser.parse_args(argv)
    rendered = render_all(load_schema())
    stale = False
    for path, source in rendered.items():
        if args.check:
            if not generated_file_matches(path, source):
                print(
                    f"STALE generated file: {path.relative_to(ROOT)}", file=sys.stderr
                )
                stale = True
        else:
            write_generated_text(path, source)
    return int(stale)


if __name__ == "__main__":
    raise SystemExit(main())

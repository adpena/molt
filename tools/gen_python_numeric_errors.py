#!/usr/bin/env python3
"""Project numeric exceptions for admitted targets.

Capture a pinned pending release with --capture-observations MAJOR.MINOR
--python-executable /absolute/path --report evidence.json. Capture writes
observations only; it neither updates policy nor admits compilation targets.
"""

from __future__ import annotations

from dataclasses import dataclass, asdict
import argparse
import hashlib
from types import MappingProxyType
from typing import Mapping
import json
from pathlib import Path
import re
import sys
import tomllib

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.generator_io import generator_main  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]

from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS  # noqa: E402

SOURCE = ROOT / "config/python_numeric_errors.toml"
OUTPUTS = tuple(
    ROOT / p
    for p in (
        "runtime/molt-runtime-core/src/numeric_error_policy_generated.rs",
        "runtime/molt-backend-luau/src/luau/numeric_error_policy_generated.rs",
        "runtime/molt-backend-rust/src/rust/numeric_error_policy_generated.rs",
    )
)
TARGETS = SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
CONTEXT_IDENTITY_FIELDS = frozenset(
    {
        "name",
        "operation",
        "operand_kind",
        "error_class",
    }
)
OPERAND_DOMAINS = {
    "truediv": {"int", "float", "complex"},
    "floordiv": {"int", "float"},
    "mod": {"int", "float"},
    "divmod": {"int", "float"},
    "pow": {"real", "complex"},
    "pow_mod": {"int"},
}


class SchemaError(ValueError):
    pass


@dataclass(frozen=True)
class Context:
    name: str
    operation: str
    operand_kind: str
    error_class: str
    messages: Mapping[str, str]

    @property
    def wire_key(self) -> str:
        return f"{self.operation}:{self.operand_kind}"


def load_policy(path: Path = SOURCE) -> tuple[Context, ...]:
    from tools.cpython_regrtest import load_cpython_sources

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if set(data) != {"schema_version", "observed_targets", "context"}:
        raise SchemaError("numeric policy fields are not the closed schema")
    if type(data["schema_version"]) is not int or data["schema_version"] != 2:
        raise SchemaError("unsupported numeric policy schema")
    observed = data["observed_targets"]
    if (
        not isinstance(observed, list)
        or any(not isinstance(target, str) for target in observed)
        or len(set(observed)) != len(observed)
        or not set(observed) <= load_cpython_sources().keys()
    ):
        raise SchemaError(
            "numeric observations require unique source-pinned coordinates"
        )
    if not set(TARGETS) <= set(observed):
        raise SchemaError("admitted target catalog needs observed numeric coordinates")
    message_fields = tuple(f"python_{target.replace('.', '_')}" for target in observed)
    context_fields = CONTEXT_IDENTITY_FIELDS | frozenset(message_fields)
    contexts = data["context"]
    if not isinstance(contexts, list) or not contexts:
        raise SchemaError("numeric context domain must be nonempty")
    result = []
    seen_names: set[str] = set()
    seen_pairs: set[tuple[str, str]] = set()
    for row in contexts:
        if not isinstance(row, dict) or set(row) != context_fields:
            raise SchemaError(
                "numeric context fields or target rows are incomplete/unknown"
            )
        if any(not isinstance(v, str) or not v for v in row.values()):
            raise SchemaError("numeric context facts must be nonempty strings")
        name, operation, kind = row["name"], row["operation"], row["operand_kind"]
        if re.fullmatch(r"[A-Z][A-Za-z0-9]*", name) is None or name in seen_names:
            raise SchemaError("numeric context names must be unique Rust identifiers")
        if operation not in OPERAND_DOMAINS or kind not in OPERAND_DOMAINS[operation]:
            raise SchemaError(
                "numeric operand context is not an admitted semantic domain"
            )
        if (operation, kind) in seen_pairs:
            raise SchemaError("duplicate numeric operation/operand context")
        expected_class = "ValueError" if operation == "pow_mod" else "ZeroDivisionError"
        if row["error_class"] != expected_class:
            raise SchemaError("numeric context changed its exception-class domain")
        seen_names.add(name)
        seen_pairs.add((operation, kind))
        result.append(
            Context(
                name,
                operation,
                kind,
                row["error_class"],
                MappingProxyType(
                    {
                        target: row[field]
                        for target, field in zip(observed, message_fields, strict=True)
                    }
                ),
            )
        )
    expected_pairs = {
        (op, kind) for op, kinds in OPERAND_DOMAINS.items() for kind in kinds
    }
    if seen_pairs != expected_pairs:
        raise SchemaError("numeric semantic operand family is incomplete")
    return tuple(result)


def render(
    contexts: tuple[Context, ...],
    *,
    integer_helper: bool = True,
) -> str:
    lines = [
        "// @generated by tools/gen_python_numeric_errors.py; DO NOT EDIT.",
        "// Source: config/python_numeric_errors.toml. Unknown target coordinates fail closed.",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]",
        "pub enum NumericErrorContext {",
        *(f"    {c.name}," for c in contexts),
        "}",
        "",
        "impl NumericErrorContext {",
        f"    pub const ALL: [Self; {len(contexts)}] = [",
        *(f"        Self::{c.name}," for c in contexts),
        "    ];",
        "",
        "    pub const fn wire_key(self) -> &'static str {",
        "        match self {",
        *(f"            Self::{c.name} => {json.dumps(c.wire_key)}," for c in contexts),
        "        }",
        "    }",
        "",
        "    pub const fn error_class(self) -> &'static str {",
        "        match self {",
        *(
            f"            Self::{c.name} => {json.dumps(c.error_class)},"
            for c in contexts
        ),
        "        }",
        "    }",
        "",
        "    pub const fn message(self, major: i64, minor: i64) -> Option<&'static str> {",
        "        match (major, minor) {",
    ]
    for target in TARGETS:
        major, minor = (int(part) for part in target.split("."))
        lines += [f"            ({major}, {minor}) => Some(match self {{"]
        lines += [
            f"                Self::{c.name} => {json.dumps(c.messages[target], ensure_ascii=False)},"
            for c in contexts
        ]
        lines += ["            }),"]
    lines += ["            _ => None,", "        }", "    }", "}"]
    if integer_helper:
        lines += ["", INTEGER_DIVMOD, FLOAT_DIVMOD]
    else:
        lines += ["", 'pub const FLOAT_DIVMOD_LUA: &str = r#"' + LUA_DIVMOD + '"#;']
    return "\n".join(lines).rstrip() + "\n"


# This integer algorithm is projected with the policy so host and standalone
# transpilers share Python's sign convention, never Euclidean/truncating division.
INTEGER_DIVMOD = """/// Python floor quotient and divisor-signed remainder for bounded integer operands.
/// None preserves explicit zero/representation-overflow admission boundaries.
pub const fn python_integer_divmod(a: i128, b: i128) -> Option<(i128, i128)> {
    let Some(mut q) = a.checked_div(b) else {
        return None;
    };
    let Some(mut r) = a.checked_rem(b) else {
        return None;
    };
    if r != 0 && (r < 0) != (b < 0) {
        q -= 1;
        r += b;
    }
    Some((q, r))
}
"""


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    contexts = load_policy()
    return {
        path: render(contexts, integer_helper="molt-backend-luau" not in path.parts)
        for path in OUTPUTS
    }


# Operations are independent observations, not expected-message lookups. The
# policy's closed wire keys validate this probe family before publishing evidence.
OBSERVATION_PROGRAM = r"""
import json, platform, sys
operations = {
    "truediv:int": lambda: 1 / 0,
    "truediv:float": lambda: 1.0 / 0.0,
    "truediv:complex": lambda: 1j / 0j,
    "floordiv:int": lambda: 1 // 0,
    "floordiv:float": lambda: 1.0 // 0.0,
    "mod:int": lambda: 1 % 0,
    "mod:float": lambda: 1.0 % 0.0,
    "divmod:int": lambda: divmod(1, 0),
    "divmod:float": lambda: divmod(1.0, 0.0),
    "pow:real": lambda: 0.0 ** -1,
    "pow:complex": lambda: 0j ** -1,
    "pow_mod:int": lambda: pow(1, 2, 0),
}
rows = {}
for key, operation in operations.items():
    try:
        operation()
    except Exception as error:
        rows[key] = {"error_class": type(error).__name__, "message": str(error)}
    else:
        raise RuntimeError("numeric observation did not raise: " + key)
print(json.dumps({"implementation": sys.implementation.name,
                  "version": platform.python_version(), "rows": rows}))
"""


def capture_observations(target: str, executable: Path) -> dict[str, object]:
    """Capture one pinned interpreter; do not admit a target or edit policy."""
    from packaging.version import Version
    from molt.toolchain_identity import capture_stable_regular_file
    from molt.python_environment_location import environment_bytecode_arguments
    from tools.command_execution import CommandExecutor
    from tools.cpython_regrtest import CPYTHON_SOURCES, load_cpython_sources
    from tools.gen_stdlib_module_union import SNAPSHOT_PATH, pinned_snapshots

    executable = executable.resolve(strict=True)
    inputs = (executable, CPYTHON_SOURCES, SNAPSHOT_PATH, SOURCE)
    before = {
        path: capture_stable_regular_file(path, label="numeric observation input")[0]
        for path in inputs
    }
    pins = load_cpython_sources()
    if target not in pins:
        raise SchemaError("numeric observation requires a canonical CPython source pin")
    pinned_snapshots((target,))
    contexts = load_policy()
    completed = CommandExecutor.for_file(__file__).run(
        [
            str(executable),
            *environment_bytecode_arguments(Path(sys.prefix)),
            "-I",
            "-c",
            OBSERVATION_PROGRAM,
        ],
        capture_output=True,
        encoding="utf-8",
        check=True,
        timeout=30,
    )
    observed = json.loads(completed.stdout)
    if observed.get("implementation") != "cpython" or Version(
        observed.get("version", "0")
    ) != Version(pins[target].tag):
        raise SchemaError(
            "observed interpreter does not match the exact pinned release"
        )
    rows = observed.get("rows")
    if not isinstance(rows, dict) or set(rows) != {row.wire_key for row in contexts}:
        raise SchemaError("numeric observation operand family is incomplete")
    for row in rows.values():
        if (
            not isinstance(row, dict)
            or set(row) != {"error_class", "message"}
            or any(not isinstance(value, str) or not value for value in row.values())
        ):
            raise SchemaError("numeric observation requires actual exception facts")
    for path, identity in before.items():
        if (
            capture_stable_regular_file(path, label="numeric observation input")[0]
            != identity
        ):
            raise RuntimeError("numeric observation inputs changed during capture")
    return {
        "schema": "molt.numeric-observations.v1",
        "target": target,
        "source": asdict(pins[target]),
        "executable": str(executable),
        "inputs": {str(path): identity.sha256 for path, identity in before.items()},
        "program_sha256": hashlib.sha256(OBSERVATION_PROGRAM.encode()).hexdigest(),
        "stdout_sha256": hashlib.sha256(completed.stdout.encode()).hexdigest(),
        "stderr_sha256": hashlib.sha256((completed.stderr or "").encode()).hexdigest(),
        "observation": observed,
        "admitted_target": target in TARGETS,
        "interpretation": "CPython observations only; no Molt execution or target admission",
    }


def main(argv: list[str] | None = None) -> int:
    from tools.generator_io import write_generated_text

    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="check generated consumers")
    mode.add_argument("--write", action="store_true", help="rewrite stale consumers")
    mode.add_argument("--capture-observations", metavar="MAJOR.MINOR")
    parser.add_argument("--python-executable", type=Path)
    parser.add_argument("--report", type=Path)
    capture = parser.parse_args(argv)
    if capture.capture_observations is not None:
        if capture.python_executable is None or capture.report is None:
            parser.error("capture requires --python-executable and --report")
        from tools.cpython_regrtest import CPYTHON_SOURCES
        from tools.gen_stdlib_module_union import SNAPSHOT_PATH

        protected = (
            SOURCE,
            CPYTHON_SOURCES,
            SNAPSHOT_PATH,
            Path(__file__),
            capture.python_executable,
            *OUTPUTS,
        )
        if capture.report.resolve() in {path.resolve() for path in protected} or (
            capture.report.exists()
            and any(
                path.exists() and capture.report.samefile(path) for path in protected
            )
        ):
            parser.error(
                "observation report must not replace a capture input or generated consumer"
            )
        result = capture_observations(
            capture.capture_observations, capture.python_executable
        )
        write_generated_text(
            capture.report, json.dumps(result, indent=2, sort_keys=True) + "\n"
        )
        return 0
    if capture.python_executable is not None or capture.report is not None:
        parser.error("--python-executable and --report require --capture-observations")
    return generator_main(
        generated_outputs,
        ["--check" if capture.check else "--write"],
        description=__doc__,
    )


FLOAT_DIVMOD = r"""/// CPython-compatible fmod-derived floor quotient and divisor-signed remainder.
/// The zero divisor remains a typed caller-owned exception boundary.
pub fn python_float_divmod(a: f64, b: f64) -> Option<(f64, f64)> {
    if b == 0.0 {
        return None;
    }
    let mut remainder = a % b;
    let mut quotient = (a - remainder) / b;
    if remainder != 0.0 {
        if (remainder < 0.0) != (b < 0.0) {
            remainder += b;
            quotient -= 1.0;
        }
    } else {
        remainder = 0.0f64.copysign(b);
    }
    let floor = if quotient != 0.0 {
        let q = quotient.floor();
        if quotient - q > 0.5 { q + 1.0 } else { q }
    } else {
        0.0f64.copysign(a / b)
    };
    Some((floor, remainder))
}

"""

LUA_DIVMOD = r"""local function molt_float_divmod(a: number, b: number): (number, number)
    local remainder = math.fmod(a, b)
    local quotient = (a - remainder) / b
    if remainder ~= 0 then
        if (remainder < 0) ~= (b < 0) then remainder += b; quotient -= 1 end
    else remainder = if b < 0 then -0.0 else 0.0 end
    local floor
    if quotient ~= 0 then
        floor = math.floor(quotient)
        if quotient - floor > 0.5 then floor += 1 end
    else
        local real_quotient = a / b
        floor = if real_quotient < 0 or 1 / real_quotient < 0 then -0.0 else 0.0
    end
    return floor, remainder
end
local function molt_float_floor_div(a: number, b: number): number
    local quotient = molt_float_divmod(a, b)
    return quotient
end
local function molt_float_mod(a: number, b: number): number
    local _, remainder = molt_float_divmod(a, b)
    return remainder
end
"""

if __name__ == "__main__":
    raise SystemExit(main())

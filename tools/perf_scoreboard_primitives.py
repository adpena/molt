"""The scoreboard's instruction-slope axis (HF-459), not a release-time board.

Programs descend from the integrator's 2026-10-31 copy primitive experiment
and 2026-11-01 construction experiment. N is supplied at execution, never at
compilation. Counts include loop/control work; no empty-loop subtraction.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
from contextlib import ExitStack
from dataclasses import asdict, dataclass
from enum import StrEnum
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import sys
import time
from typing import Any

from tools import bench, harness_memory_guard, opt_explorer
from tools.perf_authority import non_canonical_provenance
from tools.perf_scoreboard_measure import _cpython_run_env
from tools.perf_scoreboard_build_profiles import (
    profile_binding_problems,
    profile_selection_for_backend,
)
from molt.toolchain_identity import executable_content_identity

ROOT = Path(__file__).resolve().parents[1]
RATCHET = ROOT / "bench" / "primitive_ratchet.json"
SCHEMA = "molt-primitive-instructions-v1"
MIN_REPEATS = 5
CPYTHON_VERSION = (3, 12, 16)
CPYTHON_SELECTOR = ".".join(map(str, CPYTHON_VERSION))


@dataclass(frozen=True)
class Primitive:
    name: str
    body: str
    iterations: int = 1_000_000
    divisor: int = 1
    setup: str = ""
    result: str = "run(n)"

    def program(self) -> str:
        return (
            "import sys\n"
            + self.setup
            + "\ndef run(n):\n"
            + "\n".join("    " + line for line in self.body.splitlines())
            + "\n\nn = int(sys.argv[1])\nprint("
            + self.result
            + ")\n"
        )

    def operations(self) -> int:
        return self.iterations // self.divisor


_COPY_SETUP = """
class Plain:
    def __init__(self):
        self.a = 1

def two_args(x, memo):
    return x

DISPATCH = {int: two_args, str: two_args, Plain: two_args}
OBJS = [Plain() for _ in range(64)]
NIL = []
"""

_ID_ORACLE = """
def check_checksum(total, n):
    ids = [id(obj) for obj in OBJS]
    if len(set(ids)) != len(OBJS):
        return False
    expected = 0
    # The 64-object cycle cancels after two passes. Calculate the reference
    # outside the measured loop, retaining the full loop result as an input.
    for i in range(n % 128):
        expected ^= ids[i & 63]
    return total == expected
"""

# One declaration owns the complete set, workload bytes and normalization.
# reduce_ex/new_and_update count outer operations, recursion counts a depth-100
# traversal. Each legacy loop retains its original body and loop divisor.
PRIMITIVES = (
    Primitive(
        "loop",
        "total = 0\nfor i in range(n):\n    total += i\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "id",
        "total = 0\nobjs = OBJS\nfor i in range(n):\n    total ^= id(objs[i & 63])\nreturn total",
        setup=_COPY_SETUP + _ID_ORACLE,
        result="check_checksum(run(n), n)",
    ),
    Primitive(
        "memo_get",
        "memo = {id(o): o for o in OBJS}\nobjs = OBJS\nhits = 0\nfor i in range(n):\n    if memo.get(id(objs[i & 63]), NIL) is not NIL:\n        hits += 1\nreturn hits",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "memo_set",
        "memo = {}\nobjs = OBJS\nfor i in range(n):\n    memo[id(objs[i & 63])] = i\nreturn len(memo)",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "type_dispatch",
        "objs = OBJS\nfound = 0\nfor i in range(n):\n    if DISPATCH.get(type(objs[i & 63])) is not None:\n        found += 1\nreturn found",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "getattr_miss",
        'objs = OBJS\nmisses = 0\nfor i in range(n):\n    if getattr(objs[i & 63], "__deepcopy__", None) is None:\n        misses += 1\nreturn misses',
        setup=_COPY_SETUP,
    ),
    Primitive(
        "dynamic_call",
        "f = DISPATCH[int]\ntotal = 0\nfor i in range(n):\n    total += f(i, None)\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "isinstance",
        "objs = OBJS\ncount = 0\nfor i in range(n):\n    if isinstance(objs[i & 63], str):\n        count += 1\nreturn count",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "tuple_unpack",
        "total = 0\nfor i in range(n):\n    rv = (i, None, i, None, None)\n    a, b, c, d, e = rv\n    total += a + c\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "reduce_ex",
        "objs = OBJS\ncount = 0\nfor i in range(n // 10):\n    count += len(objs[i & 63].__reduce_ex__(4))\nreturn count",
        divisor=10,
        setup=_COPY_SETUP,
    ),
    Primitive(
        "new_and_update",
        'state = {"a": 1}\ncount = 0\nfor i in range(n // 10):\n    y = Plain.__new__(Plain)\n    y.__dict__.update(state)\n    count += y.a\nreturn count',
        divisor=10,
        setup=_COPY_SETUP,
    ),
    Primitive(
        "recursion",
        "total = 0\nfor _ in range(n // 100):\n    total += recurse(100)\nreturn total",
        divisor=100,
        setup=_COPY_SETUP
        + "\ndef recurse(n):\n    if n == 0:\n        return 0\n    return recurse(n - 1) + 1\n",
    ),
    Primitive(
        "plain_construction",
        "p = Point(-1, 2)\nfor i in range(n):\n    p = Point(i, 2)\nreturn p",
        iterations=100_000,
        setup="\nclass Point:\n    def __init__(self, x, y):\n        self.x = x\n        self.y = y\n",
        result="(lambda p: (p.x, p.y))(run(n))",
    ),
    Primitive(
        "dataclass_construction",
        "p = Point(-1, 2)\nfor i in range(n):\n    p = Point(i, 2)\nreturn p",
        iterations=100_000,
        setup="\nfrom dataclasses import dataclass\n\n@dataclass\nclass Point:\n    x: int\n    y: int\n",
        result="(lambda p: (p.x, p.y))(run(n))",
    ),
    Primitive(
        "raise_user",
        'count = 0\nfor _ in range(n):\n    try:\n        raise ValueError("x")\n    except ValueError:\n        count += 1\nreturn count',
    ),
    Primitive(
        "raise_runtime",
        'count = 0\nfor _ in range(n):\n    try:\n        int("abc")\n    except ValueError:\n        count += 1\nreturn count',
    ),
)


class Counter(StrEnum):
    MACOS = "macos-ri_instructions"
    PERF = "linux-perf-instructions:u"
    CALLGRIND = "callgrind-Ir-model"


@dataclass(frozen=True)
class Observation:
    instructions: int
    wall_s: float
    stdout: str
    stderr: str


@dataclass(frozen=True)
class Axis:
    primitive: str
    axis: str
    molt: float
    cpython: float
    ratio: float
    relative_spread: float
    ratios: tuple[float, ...]

    @property
    def key(self) -> str:
        return f"{self.primitive}/{self.axis}"


def _positive(value: Any, label: str) -> float:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or value <= 0
    ):
        raise ValueError(f"{label} must be finite and positive")
    return float(value)


def summarize(name: str, axis: str, pairs: list[tuple[float, float]]) -> Axis:
    if len(pairs) < MIN_REPEATS:
        raise ValueError(f"need at least {MIN_REPEATS} pairs to measure noise")
    molt = [_positive(m, "Molt count") for m, _ in pairs]
    cpython = [_positive(c, "CPython count") for _, c in pairs]
    ratios = tuple(m / c for m, c in zip(molt, cpython, strict=True))
    ratio = statistics.median(ratios)
    spread = max(abs(r / ratio - 1) for r in ratios)
    return Axis(
        name,
        axis,
        statistics.median(molt),
        statistics.median(cpython),
        ratio,
        spread,
        ratios,
    )


def expected_keys() -> set[str]:
    return {f"{p.name}/{a}" for p in PRIMITIVES for a in ("startup", "per_op")} | {
        "empty/startup"
    }


def compare_ratchet(axes: list[Axis], baseline: dict, identity: dict) -> list[str]:
    if baseline.get("schema") != SCHEMA or baseline.get("identity") != identity:
        raise ValueError(
            "ratchet schema/measurement identity differs; remeasure this cell explicitly"
        )
    rows = baseline.get("axes")
    if (
        not isinstance(rows, dict)
        or set(rows) != expected_keys()
        or {a.key for a in axes} != expected_keys()
        or len(axes) != len(rows)
    ):
        raise ValueError(
            "ratchet and measurement must cover the exact primitive axes once"
        )
    if type(baseline.get("repeats")) is not int or baseline["repeats"] < MIN_REPEATS:
        raise ValueError("ratchet lacks repeated calibration")
    problems = []
    for axis in axes:
        row = rows[axis.key]
        if not isinstance(row, dict) or set(row) != {"ratio", "noise_bound"}:
            raise ValueError(f"{axis.key}: ratio/noise record is not exact")
        ratio = _positive(row["ratio"], axis.key)
        bound = row["noise_bound"]
        if (
            isinstance(bound, bool)
            or not isinstance(bound, (int, float))
            or not math.isfinite(bound)
            or not 0 <= bound < 1
        ):
            raise ValueError("noise bound must be finite and in [0, 1)")
        _positive(axis.ratio, axis.key)
        if axis.relative_spread > bound:
            problems.append(
                f"NOISY {axis.key}: spread {axis.relative_spread:.3%} exceeds {bound:.3%}"
            )
        if axis.ratio > ratio * (1 + bound):
            problems.append(
                f"REGRESSION {axis.key}: {axis.ratio:.6g} > {ratio:.6g} + {bound:.3%}"
            )
        elif axis.ratio < ratio * (1 - bound):
            problems.append(
                f"UPDATE_REQUIRED {axis.key}: {axis.ratio:.6g} improved from {ratio:.6g}"
            )
    return problems


def make_ratchet(axes: list[Axis], identity: dict, repeats: int) -> dict:
    if len(axes) != len(expected_keys()) or {a.key for a in axes} != expected_keys():
        raise ValueError("only a complete table can update the ratchet")
    # Two independent runs can sit at opposite edges of the observed envelope.
    # Round upward to the next 0.1 percentage point, with no arbitrary floor.
    rows = {}
    for axis in axes:
        noise = math.ceil(2 * axis.relative_spread * 1000) / 1000
        if not 0 <= noise < 1:
            raise ValueError("calibration noise is unbounded; inspect raw observations")
        rows[axis.key] = {
            "ratio": _positive(axis.ratio, axis.key),
            "noise_bound": noise,
        }
    return {
        "schema": SCHEMA,
        "provenance": non_canonical_provenance(
            requested_profile="release", source="tools/perf_scoreboard.py primitives"
        ),
        "identity": identity,
        "repeats": repeats,
        "noise_bound": max(row["noise_bound"] for row in rows.values()),
        "noise_rule": "per axis: 2 * maximum paired-ratio deviation from median, rounded up to 0.001; zero is exact",
        "axes": rows,
    }


def parse_count(counter: Counter, text: str) -> int:
    if counter is Counter.MACOS:
        values = re.findall(r"^\s*(\d+)\s+instructions retired\s*$", text, re.M)
    elif counter is Counter.PERF:
        # perf -x ';' --no-big-num: value;unit;event;runtime;percent-running
        values = []
        for line in text.splitlines():
            fields = line.split(";")
            if len(fields) >= 5 and fields[2] == "instructions:u":
                if float(fields[4].strip().rstrip("%")) != 100:
                    raise ValueError(
                        "multiplexed or invalid instruction count is not admissible"
                    )
                values.append(fields[0].strip())
    else:
        events = re.findall(r"^events:\s*(.+)$", text, re.M)
        summaries = re.findall(r"^summary:\s*(.+)$", text, re.M)
        if len(events) != 1 or len(summaries) != 1 or "Ir" not in events[0].split():
            raise ValueError("Callgrind needs one Ir event summary")
        values = [summaries[0].split()[events[0].split().index("Ir")]]
    if len(values) != 1 or not values[0].isdigit() or int(values[0]) <= 0:
        raise ValueError(f"missing, ambiguous or invalid {counter} instruction count")
    return int(values[0])


def select_counter(request: str) -> Counter:
    if request != "auto":
        chosen = Counter(request)
    elif sys.platform == "darwin":
        chosen = Counter.MACOS
    elif sys.platform == "linux":
        # Select before workload/build launches, never retry a failed measurement
        # with another model. VM PMU/permission absence selects a labelled model.
        paranoid = Path("/proc/sys/kernel/perf_event_paranoid")
        pmus = Path("/sys/bus/event_source/devices")
        exposed = pmus.is_dir() and any(
            p.name == "cpu" or p.name.startswith("armv8_pmuv3") for p in pmus.iterdir()
        )
        permitted = (
            paranoid.is_file()
            and int(paranoid.read_text(encoding="ascii").strip()) <= 2
        )
        chosen = (
            Counter.PERF
            if exposed and permitted and shutil.which("perf")
            else Counter.CALLGRIND
        )
    else:
        raise ValueError(f"no instruction-counter plan for {sys.platform}")
    tool = {
        Counter.MACOS: "/usr/bin/time",
        Counter.PERF: "perf",
        Counter.CALLGRIND: "valgrind",
    }[chosen]
    if not shutil.which(tool):
        raise ValueError(
            f"{chosen} requires {tool}; no measurement or ratchet substitution is allowed"
        )
    if chosen is Counter.MACOS and sys.platform != "darwin":
        raise ValueError("ri_instructions requires macOS")
    if chosen is not Counter.MACOS and sys.platform != "linux":
        raise ValueError("perf/Callgrind plans require Linux")
    return chosen


def observe(
    counter: Counter, command: list[str], prefix: Path, env: dict, timeout: float
) -> Observation:
    report = prefix.with_suffix(".counter.txt")
    if report.exists():
        raise ValueError(f"measurement output already exists: {report}")
    if counter is Counter.MACOS:
        argv = ["/usr/bin/time", "-l", "-o", str(report), *command]
    elif counter is Counter.PERF:
        argv = [
            "perf",
            "stat",
            "--no-big-num",
            "-x",
            ";",
            "-e",
            "instructions:u",
            "-o",
            str(report),
            "--",
            *command,
        ]
    else:
        argv = [
            "valgrind",
            "--tool=callgrind",
            "--collect-atstart=yes",
            f"--callgrind-out-file={report}",
            f"--log-file={prefix.with_suffix('.tool.txt')}",
            *command,
        ]
    result = harness_memory_guard.guarded_completed_process(
        argv,
        prefix="MOLT_BENCH",
        env=env,
        cwd=ROOT,
        timeout=timeout,
        text=True,
        errors="strict",
    )
    prefix.with_suffix(".stdout.txt").write_text(result.stdout or "", encoding="utf-8")
    prefix.with_suffix(".stderr.txt").write_text(result.stderr or "", encoding="utf-8")
    if (
        result.returncode != 0
        or result.timed_out
        or result.infrastructure_failure
        or result.orphaned_process_groups
    ):
        raise RuntimeError(
            f"instruction measurement failed: {argv!r}; rc={result.returncode}; see {prefix}"
        )
    count = parse_count(counter, report.read_text(encoding="utf-8"))
    return Observation(
        count,
        result.child_elapsed_s or result.elapsed_s or 0.0,
        result.stdout or "",
        result.stderr or "",
    )


def _dump(path: Path, value: Any) -> None:
    from molt.cli.atomic_io import _atomic_write_text

    _atomic_write_text(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def primitive_build_env(
    base: Mapping[str, str], lane_env: Mapping[str, str], program_dir: Path
) -> dict[str, str]:
    """The build environment of one primitive program.

    The program's own directory is its project. Otherwise the build adopts the
    nearest enclosing project (a checkout holding ``--out-dir``) and compiles
    the primitive against that project's sources and environment.
    """
    env = dict(base, PYTHONHASHSEED="0", MOLT_KEEP_SYMBOLS="1")
    env.update(lane_env)
    env["MOLT_PROJECT_ROOT"] = str(program_dir)
    return env


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out-dir", required=True, type=Path, help="new retained evidence directory"
    )
    parser.add_argument(
        "--cpython", required=True, help="exact CPython 3.12.16 executable"
    )
    parser.add_argument("--ratchet", type=Path, default=RATCHET)
    parser.add_argument("--update-ratchet", action="store_true")
    parser.add_argument("--repeats", type=int, default=MIN_REPEATS)
    parser.add_argument("--counter", choices=["auto", *Counter], default="auto")
    parser.add_argument(
        "--profile-costs",
        action="store_true",
        help="opt_explorer sample buckets for each primitive (macOS)",
    )
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args(argv)
    if args.repeats < MIN_REPEATS:
        parser.error(f"--repeats must be at least {MIN_REPEATS}")
    try:
        return run(args)
    except (ValueError, RuntimeError, OSError) as exc:
        print(f"primitive-instructions: {exc}", file=sys.stderr)
        return 2


def run(args: argparse.Namespace) -> int:
    started = time.monotonic()
    counter = select_counter(args.counter)
    if args.profile_costs and sys.platform != "darwin":
        raise ValueError("--profile-costs needs opt_explorer's macOS sampler")
    env = dict(_cpython_run_env())
    env.update(PYTHONHASHSEED="0", LC_ALL="C")
    from molt.python_environment_identity import PythonIdentityVerifier
    from molt.python_interpreter import (
        PythonInterpreterRequest,
        resolve_python_interpreter,
    )

    with PythonIdentityVerifier() as verifier:
        interpreter = resolve_python_interpreter(
            PythonInterpreterRequest(*CPYTHON_VERSION),
            override=None if args.cpython == CPYTHON_SELECTOR else args.cpython,
            verifier=verifier,
            env=env,
            cwd=ROOT,
        )
    if len(interpreter.command) != 1:
        raise ValueError("primitive denominator requires one physical executable")
    # uv discovery may select an existing virtualenv symlink. The denominator
    # is the physical base executable, independent of that activation path.
    oracle = Path(interpreter.command[0]).resolve(strict=True)
    probe = harness_memory_guard.guarded_completed_process(
        [
            str(oracle),
            "-I",
            "-S",
            "-c",
            "import platform; print(platform.python_implementation(), platform.python_version())",
        ],
        prefix="MOLT_BENCH",
        env=env,
        timeout=30,
    )
    if probe.returncode or probe.stdout.strip() != f"CPython {CPYTHON_SELECTOR}":
        raise ValueError(
            f"denominator must be CPython 3.12.16, got {probe.stdout!r} {probe.stderr!r}"
        )
    out = args.out_dir.expanduser().absolute()
    out.mkdir(parents=True, exist_ok=False)
    identity = {
        "os": platform.system(),
        "arch": platform.machine(),
        "counter": counter.value,
        "cpython": CPYTHON_SELECTOR,
        "cpython_image": executable_content_identity(oracle, label="CPython oracle"),
        "profile": "release",
        "workloads_sha256": hashlib.sha256(
            "".join(
                p.program() + str(p.iterations) + str(p.divisor) for p in PRIMITIVES
            ).encode()
        ).hexdigest(),
    }
    # Resolve release through the same lane authority as the wall-time board.
    lane = profile_selection_for_backend("native", "release-output")
    identity["selected_profiles"] = lane.as_record()
    _dump(out / "measurement-identity.json", identity)
    baseline = None
    if not args.update_ratchet:
        baseline = json.loads(args.ratchet.read_text(encoding="utf-8"))
        if not isinstance(baseline, dict):
            raise ValueError("ratchet must be a JSON object")
        if baseline.get("schema") != SCHEMA or baseline.get("identity") != identity:
            raise ValueError(
                f"ratchet schema/measurement identity differs before build; see {out / 'measurement-identity.json'}"
            )
        if set(baseline.get("axes", {})) != expected_keys():
            raise ValueError("ratchet must cover the exact primitive axes")
    # Prove the selected plan with the actual guest environment before any
    # expensive build. A denied PMU ends this run; it never selects another model.
    observe(
        counter,
        [str(oracle), "-I", "-S", "-c", "pass"],
        out / "counter-preflight",
        env,
        min(args.timeout, 30),
    )
    axes: list[Axis] = []
    rows = []
    programs = [Primitive("empty", "return n", iterations=1), *PRIMITIVES]
    for primitive in programs:
        print(
            f"[primitive] {primitive.name}: build once, {args.repeats} paired samples",
            flush=True,
        )
        source = out / f"{primitive.name}.py"
        source.write_text(primitive.program(), encoding="utf-8")
        if lane.backend != "native":
            raise ValueError("primitive build must use native backend")
        build_env = primitive_build_env(os.environ, lane.environment(), out)
        binary = bench.prepare_molt_binary(
            str(source),
            extra_args=["--python-version", "3.12", "--backend", lane.codegen_backend],
            env=build_env,
            build_profile="release",
        )
        if isinstance(binary, bench.MoltFailure):
            _dump(out / "failure.json", bench.molt_failure_payload(binary))
            raise RuntimeError(
                f"{primitive.name} build failed; see {out / 'failure.json'}"
            )
        row: dict[str, Any] = {
            "primitive": primitive.name,
            "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            "n": primitive.iterations,
            "operations": primitive.operations(),
            "build_s": binary.build_s,
            "build_observation": binary.build_observation,
            "samples": [],
        }
        with ExitStack() as cleanup:
            cleanup.callback(binary.temp_dir.cleanup)
            problems = profile_binding_problems(
                binary.build_observation, backend="native", profile=lane.runtime_profile
            )
            if problems:
                raise ValueError(f"{primitive.name}: {problems}")
            row["binary_identity"] = executable_content_identity(
                binary.path, label="primitive binary"
            )
            startup_pairs = []
            slope_pairs = []
            for repeat in range(args.repeats):
                observations = {}
                # Reverse runtime order on alternating pairs; startup is always
                # measured separately, not inferred from a wall-clock fit.
                for runtime in (
                    ("cpython", "molt") if repeat % 2 else ("molt", "cpython")
                ):
                    for n in (
                        [0] if primitive.name == "empty" else [0, primitive.iterations]
                    ):
                        command = (
                            [str(binary.path), str(n)]
                            if runtime == "molt"
                            else [str(oracle), "-I", "-S", str(source), str(n)]
                        )
                        observation = observe(
                            counter,
                            command,
                            out / f"{primitive.name}-{repeat}-{runtime}-{n}",
                            env,
                            args.timeout,
                        )
                        observations[(runtime, n)] = observation
                for n in (
                    [0] if primitive.name == "empty" else [0, primitive.iterations]
                ):
                    if (
                        observations[("molt", n)].stdout
                        != observations[("cpython", n)].stdout
                        or observations[("molt", n)].stderr
                        != observations[("cpython", n)].stderr
                    ):
                        raise ValueError(
                            f"{primitive.name} N={n}: observable output differs from CPython"
                        )
                startup_pairs.append(
                    (
                        observations[("molt", 0)].instructions,
                        observations[("cpython", 0)].instructions,
                    )
                )
                if primitive.name != "empty":
                    slope_pairs.append(
                        tuple(
                            (
                                observations[(r, primitive.iterations)].instructions
                                - observations[(r, 0)].instructions
                            )
                            / primitive.operations()
                            for r in ("molt", "cpython")
                        )
                    )
                row["samples"].append(
                    {f"{r}/{n}": asdict(o) for (r, n), o in observations.items()}
                )
                _dump(out / f"{primitive.name}.json", row)
            axes.append(summarize(primitive.name, "startup", startup_pairs))
            if slope_pairs:
                axes.append(summarize(primitive.name, "per_op", slope_pairs))
            if args.profile_costs and primitive.name != "empty":
                report = opt_explorer.sample_run(
                    binary.path, 5, [str(primitive.iterations)]
                )
                (out / f"{primitive.name}.sample.txt").write_text(
                    report, encoding="utf-8"
                )
                _, profile = opt_explorer.render_profile(
                    binary.path, source.stem, report, [], 10
                )
                if not profile["samples"]:
                    raise ValueError(
                        f"{primitive.name}: profiling produced no samples; increase workload"
                    )
                row["cost_buckets"] = profile["buckets"]
        rows.append(row)
        _dump(
            out / "partial.json",
            {"identity": identity, "axes": [asdict(a) for a in axes], "programs": rows},
        )
    if args.update_ratchet:
        ratchet = make_ratchet(axes, identity, args.repeats)
        problems = []
    else:
        assert baseline is not None
        ratchet = baseline
        problems = compare_ratchet(axes, ratchet, identity)
    payload = {
        "schema": SCHEMA,
        "provenance": non_canonical_provenance(
            requested_profile="release", source="tools/perf_scoreboard.py primitives"
        ),
        "identity": identity,
        "axes": [asdict(a) for a in axes],
        "programs": rows,
        "elapsed_s": time.monotonic() - started,
        "repeats": args.repeats,
        "noise_bound": ratchet["noise_bound"],
        "problems": problems,
        "release_qualified": False,
        "v1_target_passes": all(
            a.ratio <= (1.5 if a.axis == "startup" else 1.0) for a in axes
        ),
    }
    _dump(out / "table.json", payload)
    lines = [
        "| Primitive / axis | Molt instructions | CPython instructions | Molt / CPython | Noise bound |",
        "|---|---:|---:|---:|---:|",
    ]
    for axis in axes:
        lines.append(
            f"| {axis.key} | {axis.molt:,.2f} | {axis.cpython:,.2f} | {axis.ratio:.4f} | {ratchet['axes'][axis.key]['noise_bound']:.1%} |"
        )
    lines.extend(
        [
            "",
            f"{args.repeats} paired repeats; maximum axis noise bound {ratchet['noise_bound']:.3%}; elapsed {payload['elapsed_s']:.1f}s.",
        ]
    )
    if args.profile_costs:
        lines.extend(["", "Top sampled cost buckets (including startup):"])
        for row in rows:
            if "cost_buckets" in row:
                buckets = ", ".join(
                    f"{b['bucket']} {b['share']:.1%}" for b in row["cost_buckets"][:3]
                )
                lines.append(f"- {row['primitive']}: {buckets}")
    text = "\n".join(lines) + "\n"
    (out / "table.md").write_text(text, encoding="utf-8")
    print(text)
    if args.update_ratchet:
        _dump(args.ratchet, ratchet)
    for problem in problems:
        print(problem, file=sys.stderr)
    return int(bool(problems))

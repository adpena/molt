from __future__ import annotations

import os
import sys
import textwrap
from pathlib import Path
from typing import Literal

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import (
    observe_native_test_executable,
    retain_native_test_failure,
    run_native_test_process,
)
from tests import process_guard_common
from molt.cli import setup_readiness as cli_setup_readiness

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def _compile_and_run(source: str, profile: str, *, backend: str | None = None) -> str:
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-loop-join-"
    ) as tmp_path:
        module_stem = f"loop_join_semantics_{tmp_path.name}"
        src_path = tmp_path / f"{module_stem}.py"
        source_bytes = source.encode("utf-8")
        src_path.write_bytes(source_bytes)
        binary_path = tmp_path / f"{module_stem}_molt"

        env = development_artifact_env(
            ROOT,
            os.environ,
            session_prefix=f"test-loop-join-{profile}",
            create_dirs=True,
        )
        env = env.updated({"PYTHONPATH": str(SRC_DIR)})

        build_command = [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            profile,
            *([] if backend is None else ["--backend", backend]),
            str(src_path),
            "--out-dir",
            str(tmp_path),
        ]
        run_command = [str(binary_path)]
        build = run = executable = None
        phase: Literal["build", "run"] = "build"
        try:
            build = run_native_test_process(
                build_command, cwd=ROOT, env=env, capture_output=True, text=False
            )
            assert build.returncode == 0, build.stderr.decode("utf-8", errors="replace")
            assert binary_path.exists(), f"expected binary at {binary_path}"
            executable = observe_native_test_executable(binary_path, env)
            phase = "run"
            run = run_native_test_process(
                run_command, capture_output=True, text=False, timeout=30
            )
            assert run.returncode == 0, run.stderr.decode("utf-8", errors="replace")
            # Preserve subprocess text mode's universal-newline return contract;
            # the failure bundle retains the original captured bytes.
            return (
                run.stdout.decode("utf-8", errors="replace")
                .replace("\r\n", "\n")
                .replace("\r", "\n")
                .strip()
            )
        except Exception as exc:
            retain_native_test_failure(
                exc,
                phase=phase,
                source=source_bytes,
                source_path=src_path,
                executable_path=binary_path,
                executable=executable,
                backend=backend,
                profile=profile,
                build_command=build_command,
                build_cwd=ROOT,
                run_command=run_command,
                build=build,
                run=run,
                env=env,
            )
            raise


def _llvm_backend_available() -> bool:

    major, toolchain = cli_setup_readiness._detect_llvm_backend_toolchain(ROOT)
    return major is not None and toolchain is not None


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_loop_join_semantics_match_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        def f():
            acc = 0
            i = 0
            while i < 3:
                j = 0
                while j < 4:
                    if j < 2:
                        picked = i + j
                    else:
                        picked = j + 1
                    acc = acc + picked
                    j = j + 1
                i = i + 1
            print(acc)

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_counted_store_load_loop_exit_carrier_matches_cpython(
    profile: str,
) -> None:
    source = textwrap.dedent(
        """
        def f():
            i = 0
            while i < 32:
                i = i + 1
            print(i)

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.parametrize(
    ("profile", "source"),
    [
        (
            "dev",
            textwrap.dedent(
                """
                def f():
                    total = 0.0
                    i = 0
                    while i < 5:
                        if i < 3:
                            total = total + 1.25
                        else:
                            total = total + 0.5
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
        (
            "release",
            textwrap.dedent(
                """
                def f():
                    total = 0.0
                    i = 0
                    while i < 5:
                        if i < 3:
                            total = total + 1.25
                        else:
                            total = total + 0.5
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
        (
            "dev",
            textwrap.dedent(
                """
                def f():
                    total = 0
                    i = 0
                    while i < 6:
                        if i and i < 4:
                            total = total + i
                        else:
                            total = total + 10
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
        (
            "release",
            textwrap.dedent(
                """
                def f():
                    total = 0
                    i = 0
                    while i < 6:
                        if i and i < 4:
                            total = total + i
                        else:
                            total = total + 10
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
        (
            "dev",
            textwrap.dedent(
                """
                def f():
                    try:
                        raise RuntimeError("boom")
                    except RuntimeError:
                        print("caught")

                f()
                """
            ),
        ),
        (
            "release",
            textwrap.dedent(
                """
                def f():
                    try:
                        raise RuntimeError("boom")
                    except RuntimeError:
                        print("caught")

                f()
                """
            ),
        ),
        (
            "dev",
            textwrap.dedent(
                """
                def f():
                    total = 0
                    i = 0
                    while i < 5:
                        try:
                            if i == 3:
                                raise RuntimeError("boom")
                            total = total + i
                        except RuntimeError:
                            total = total + 100
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
        (
            "release",
            textwrap.dedent(
                """
                def f():
                    total = 0
                    i = 0
                    while i < 5:
                        try:
                            if i == 3:
                                raise RuntimeError("boom")
                            total = total + i
                        except RuntimeError:
                            total = total + 100
                        i = i + 1
                    print(total)

                f()
                """
            ),
        ),
    ],
)
def test_native_runtime_regressions_match_cpython(profile: str, source: str) -> None:
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_bool_primary_loop_and_list_paths_match_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        def flip(flag):
            out = []
            i = 0
            current = flag
            values = [True, False, True, True]
            while i < 4:
                if current:
                    out.append(values[i])
                else:
                    out.append(not values[i])
                current = not current
                i = i + 1
            values[1] = current
            values[2] = not current
            print(out)
            print(values)
            return current

        print(flip(True))
        print(flip(False))
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_exception_loop_with_prints_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        def f():
            total = 0
            i = 0
            print("start", total, i)
            while i < 3:
                print("top", total, i)
                try:
                    if i == 1:
                        raise RuntimeError("boom")
                    total = total + i
                except RuntimeError:
                    total = total + 100
                print("after_try", total, i)
                i = i + 1
            print("done", total, i)

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_exception_loop_raise_accumulator_preserves_int_carrier(
    profile: str,
) -> None:
    source = textwrap.dedent(
        """
        def f():
            total = 0
            i = 0
            while i < 4:
                try:
                    if i % 3 == 0:
                        raise ValueError(i)
                    total += i
                except ValueError as e:
                    parsed = int(str(e))
                    print("parsed", parsed, type(parsed).__name__)
                    total += parsed
                    print("total", total, type(total).__name__)
                i += 1

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile) == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
def test_native_release_llvm_simple_exception_catch_matches_cpython() -> None:
    source = textwrap.dedent(
        """
        def f():
            try:
                raise RuntimeError("boom")
            except RuntimeError:
                print("caught")

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, "release", backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
def test_native_release_llvm_exception_loop_matches_cpython() -> None:
    source = textwrap.dedent(
        """
        def f():
            total = 0
            i = 0
            while i < 5:
                try:
                    if i == 3:
                        raise RuntimeError("boom")
                    total = total + i
                except RuntimeError:
                    total = total + 100
                i = i + 1
            print(total)

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, "release", backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_generator_expression_list_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        print(list(x for x in [1]))
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_generator_expression_next_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        g = (x for x in [1])
        print(next(g))
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_call_result_or_tuple_fallback_matches_cpython(
    profile: str,
) -> None:
    source = textwrap.dedent(
        """
        class T(tuple):
            __slots__ = ()

            def __new__(cls, values):
                return tuple.__new__(cls, values)

        _T = T

        def g():
            return None

        def f():
            values = g() or (1, 2)
            print(type(values).__name__)
            print(_T(values))

        f()
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_dynamic_getattr_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        class P:
            def __init__(self):
                self.foo = 7

        p = P()
        name = "foo"
        print(getattr(p, name))
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_dynamic_hasattr_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        class P:
            pass

        p = P()
        print(hasattr(p, "foo"))
        setattr(p, "foo", 7)
        print(hasattr(p, "foo"))
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_import_pathlib_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        import pathlib
        print(type(pathlib).__name__)
        print(pathlib)
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()
    expected_lines = expected.splitlines()
    actual = _compile_and_run(source, profile, backend="llvm")
    actual_lines = actual.splitlines()

    assert actual_lines[0] == expected_lines[0] == "module"
    assert actual_lines[1].startswith("<module 'pathlib'"), actual
    assert " from '" in actual_lines[1], actual


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_bool_or_matches_cpython(profile: str) -> None:
    source = textwrap.dedent(
        """
        print(None or 1)
        print(0 or 2)
        print(3 or 4)
        """
    )
    expected = run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, profile, backend="llvm") == expected


# ── Integer-overflow regression guards for the LLVM backend ──
#
# The LLVM backend used to emit raw machine `add`/`sub`/`mul` whenever both
# operands were `TirType::I64`, then mask the result to 47 bits at box time —
# with no inline-range check and no BigInt promotion. The same Python program
# therefore produced a WRONG result on the LLVM backend for any integer
# operation crossing 2**47 (e.g. a doubled accumulator silently truncated to
# 0), while the native and WASM backends promoted to BigInt. These tests build
# the program through the LLVM backend and assert the output matches BOTH
# CPython and the native (Cranelift) backend, so the divergence cannot return.
#
# The programs deliberately use `while` loops with function-local accumulators
# (no `range()` / builtin-call machinery) so they exercise the raw-i64 loop
# carrier path that exposed the miscompile.


def _cpython_output(source: str) -> str:
    return run_native_test_process(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()


@pytest.mark.parametrize("backend", ["llvm", "cranelift"])
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_integer_carrier_matrix_matches_cpython(
    profile: str, backend: str
) -> None:
    if backend == "llvm" and not _llvm_backend_available():
        pytest.skip("LLVM backend toolchain is unavailable")
    # One compiled module owns the coherent integer-carrier matrix. Keeping the
    # cases together proves the same backend artifact and avoids rebuilding the
    # runtime once per assertion.
    source = textwrap.dedent(
        """
        def crosses_47_bits():
            x = 1
            n = 0
            while n < 60:
                x = x + x
                n = n + 1
            return x

        def overflows_i64():
            x = 1
            n = 0
            while n < 70:
                x = x + x
                n = n + 1
            return x

        def sum_crosses_47_bits():
            total = 0
            i = 0
            while i < 100000:
                total = total + i * i
                i = i + 1
            return total

        def stays_inline():
            total = 0
            i = 0
            while i < 1000:
                total = total + i
                i = i + 1
            return total

        print(crosses_47_bits())
        print(overflows_i64())
        print(sum_crosses_47_bits())
        print(stays_inline())
        print(7 * 11 + 7 - 11)
        absolute = abs
        print(absolute(-crosses_47_bits()))
        print(absolute(-overflows_i64()))
        print(absolute(-17))
        """
    )
    expected = _cpython_output(source)
    expected_lines = expected.splitlines()
    assert expected_lines[:2] == [
        "1152921504606846976",
        "1180591620717411303424",
    ]
    assert int(expected_lines[2]) > (1 << 47)
    assert expected_lines[3:5] == ["499500", "73"]
    assert expected_lines[5:] == [*expected_lines[:2], "17"]
    assert _compile_and_run(source, profile, backend=backend) == expected

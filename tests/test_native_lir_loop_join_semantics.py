from __future__ import annotations

import os
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def _compile_and_run(source: str, profile: str, *, backend: str | None = None) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        module_stem = f"loop_join_semantics_{tmp_path.name}"
        src_path = tmp_path / f"{module_stem}.py"
        src_path.write_text(source)
        binary_path = tmp_path / f"{module_stem}_molt"

        env = development_artifact_env(
            ROOT,
            os.environ,
            session_prefix=f"test-loop-join-{profile}",
            session_id=f"test-loop-join-{profile}-{tmp_path.name}",
            create_dirs=True,
        )
        env["PYTHONPATH"] = str(SRC_DIR)

        build = run_native_test_process(
            [
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
            ],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=300,
        )
        assert build.returncode == 0, build.stderr
        assert binary_path.exists(), f"expected binary at {binary_path}"

        run = run_native_test_process(
            [str(binary_path)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        assert run.returncode == 0, run.stderr
        return run.stdout.strip()


def _llvm_backend_available() -> bool:
    from molt import cli as molt_cli

    major, toolchain = molt_cli._detect_llvm_backend_toolchain(ROOT)
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


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_int_accumulator_crosses_47_bits_matches_cpython_and_native(
    profile: str,
) -> None:
    # Doubling accumulator: x = 2**60 after 60 iterations, well past the 47-bit
    # inline integer payload. Pre-fix LLVM printed 0 (2**60 & (2**47 - 1)).
    source = textwrap.dedent(
        """
        def compute():
            x = 1
            n = 0
            while n < 60:
                x = x + x
                n = n + 1
            return x

        print(compute())
        """
    )
    expected = _cpython_output(source)
    assert expected == "1152921504606846976"
    assert _compile_and_run(source, profile, backend="llvm") == expected
    assert _compile_and_run(source, profile, backend="cranelift") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_int_overflows_i64_promotes_to_bigint_matches_cpython_and_native(
    profile: str,
) -> None:
    # Doubling 70 times reaches 2**70, which exceeds a signed i64. This requires
    # the non-overflow-safe arithmetic to route through the runtime (BigInt),
    # not a raw machine `add` that would wrap at 64 bits.
    source = textwrap.dedent(
        """
        def compute():
            x = 1
            n = 0
            while n < 70:
                x = x + x
                n = n + 1
            return x

        print(compute())
        """
    )
    expected = _cpython_output(source)
    assert expected == "1180591620717411303424"
    assert _compile_and_run(source, profile, backend="llvm") == expected
    assert _compile_and_run(source, profile, backend="cranelift") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_int_sum_accumulator_overflow_matches_cpython_and_native(
    profile: str,
) -> None:
    # A summing accumulator whose total crosses 2**47 mid-loop. The loop counter
    # is interval-bounded (overflow-safe, stays a raw i64), while the unbounded
    # total must stay boxed — exercising both halves of the carrier split.
    source = textwrap.dedent(
        """
        def compute():
            total = 0
            i = 0
            while i < 9000000:
                total = total + i * i
                i = i + 1
            return total

        print(compute())
        """
    )
    expected = _cpython_output(source)
    assert int(expected) > (1 << 47)
    assert _compile_and_run(source, profile, backend="llvm") == expected
    assert _compile_and_run(source, profile, backend="cranelift") == expected


@pytest.mark.skipif(
    not _llvm_backend_available(),
    reason="LLVM backend toolchain is unavailable",
)
@pytest.mark.parametrize("profile", ["dev", "release"])
def test_native_llvm_non_overflowing_int_loop_matches_cpython(profile: str) -> None:
    # Regression guard: a non-overflowing integer loop must remain correct under
    # the carrier-split fix (the raw-i64 fast path is still taken and exact).
    source = textwrap.dedent(
        """
        def compute():
            total = 0
            i = 0
            while i < 1000:
                total = total + i
                i = i + 1
            return total

        print(compute())
        print(7 * 11 + 7 - 11)
        """
    )
    expected = _cpython_output(source)
    assert expected == "499500\n73"
    assert _compile_and_run(source, profile, backend="llvm") == expected

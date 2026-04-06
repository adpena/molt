from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def _compile_and_run(source: str, profile: str, *, backend: str | None = None) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        src_path = tmp_path / "loop_join_semantics.py"
        src_path.write_text(source)
        binary_path = tmp_path / "loop_join_semantics_molt"

        env = {
            **os.environ,
            "PYTHONPATH": str(SRC_DIR),
            "MOLT_EXT_ROOT": str(ROOT),
            "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")),
            "MOLT_DIFF_CARGO_TARGET_DIR": os.environ.get(
                "MOLT_DIFF_CARGO_TARGET_DIR",
                os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")),
            ),
            "MOLT_CACHE": os.environ.get("MOLT_CACHE", str(ROOT / ".molt_cache")),
            "MOLT_DIFF_ROOT": os.environ.get("MOLT_DIFF_ROOT", str(ROOT / "tmp" / "diff")),
            "MOLT_DIFF_TMPDIR": os.environ.get("MOLT_DIFF_TMPDIR", str(ROOT / "tmp")),
            "UV_CACHE_DIR": os.environ.get("UV_CACHE_DIR", str(ROOT / ".uv-cache")),
            "TMPDIR": os.environ.get("TMPDIR", str(ROOT / "tmp")),
            "MOLT_SESSION_ID": f"test-loop-join-{profile}-{tmp_path.name}",
        }

        build = subprocess.run(
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

        run = subprocess.run(
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
    expected = subprocess.run(
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
    expected = subprocess.run(
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
    expected = subprocess.run(
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
    expected = subprocess.run(
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
    expected = subprocess.run(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()

    assert _compile_and_run(source, "release", backend="llvm") == expected

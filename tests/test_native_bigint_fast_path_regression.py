"""Regression tests for the integer scalar fast-path BigInt-operand bug.

The native (Cranelift), WASM, and LLVM backends emit a "trusted unbox" fast path
for integer scalar ops (arithmetic, bitwise, shift, comparison, unary, and
truthiness) whenever the representation plan classifies the operands as the
Python ``int`` *type*. That classification, however, includes arbitrary-precision
``int`` values, which Molt stores as heap-allocated BigInts behind a ``TAG_PTR``
NaN-box (anything whose magnitude exceeds the 47-bit inline range). The trusted
shift-unbox truncated such a pointer to garbage, so any fast-path op on a BigInt
operand returned a wrong value (and ``int``/``float`` ``==`` compared pointer
identity instead of value).

This reproduced as: an indirectly-stored function called with a BigInt argument
returned garbage (e.g. ``apply(funcs[0], 1 << 60, 7)`` yielded a small truncated
integer instead of ``1152921504606846983``). The argument flowed into the callee,
whose ``a + b`` (typed ``int``) took the trusted fast path and truncated the
BigInt.

The fix guards every integer scalar fast path on a runtime inline-int tag check,
routing BigInt / float / mixed operands to the boxed runtime helper, which is
value-correct. These tests pin CPython-exact behavior on the native backend; the
WASM split-runtime variant of the original indirect-call repro is covered by
``test_wasm_split_runtime.test_split_runtime_direct_indirect_call_uses_initialized_table_refs``.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process


def _native_env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="native-bigint-fast-path",
        session_id=os.environ.get("MOLT_SESSION_ID") or "native-bigint-fast-path",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    return env


def _run_native(root: Path, src: Path) -> "tuple[int, str, str]":
    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(src),
        ],
        cwd=root,
        env=_native_env(root),
        capture_output=True,
        text=True,
        timeout=240,
        check=False,
    )
    return run.returncode, run.stdout, run.stderr


def test_native_indirect_call_with_bigint_arg(tmp_path: Path) -> None:
    """The reported repro: an indirectly-stored function called with a BigInt
    argument must return the CPython-correct value, not the truncated garbage the
    trusted-unbox fast path produced for the callee's ``a + b``."""
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "indirect_bigint.py"
    src.write_text(
        "def apply(fn, a, b):\n"
        "    return fn(a, b)\n"
        "\n"
        "def add(a: int, b: int) -> int:\n"
        "    return a + b\n"
        "\n"
        "funcs = [add]\n"
        "print(apply(funcs[0], 1 << 60, 7))\n",
        encoding="utf-8",
    )

    rc, out, err = _run_native(root, src)
    assert rc == 0, out + err
    # 1 << 60 == 1152921504606846976; + 7 == 1152921504606846983.
    assert out.strip() == "1152921504606846983"


def test_native_bigint_operand_fast_path_parity(tmp_path: Path) -> None:
    """Every integer scalar fast path (arithmetic, bitwise, shift, comparison,
    unary, truthiness) must produce CPython-exact results for BigInt operands
    that are merely ``int``-typed, including the load-bearing ``1 << 47`` case
    whose low 47 bits are all zero (so a truncating unbox would treat it as 0)."""
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bigint_fast_path.py"
    src.write_text(
        "def addi(a: int, b: int) -> int:\n"
        "    return a + b\n"
        "def subi(a: int, b: int) -> int:\n"
        "    return a - b\n"
        "def muli(a: int, b: int) -> int:\n"
        "    return a * b\n"
        "def fdiv(a: int, b: int) -> int:\n"
        "    return a // b\n"
        "def modi(a: int, b: int) -> int:\n"
        "    return a % b\n"
        "def andi(a: int, b: int) -> int:\n"
        "    return a & b\n"
        "def ori(a: int, b: int) -> int:\n"
        "    return a | b\n"
        "def xori(a: int, b: int) -> int:\n"
        "    return a ^ b\n"
        "def lsh(a: int, b: int) -> int:\n"
        "    return a << b\n"
        "def rsh(a: int, b: int) -> int:\n"
        "    return a >> b\n"
        "def negi(a: int) -> int:\n"
        "    return -a\n"
        "def absi(a: int) -> int:\n"
        "    return abs(a)\n"
        "def inv(a: int) -> int:\n"
        "    return ~a\n"
        "def lti(a: int, b: int) -> bool:\n"
        "    return a < b\n"
        "def eqi(a: int, b: int) -> bool:\n"
        "    return a == b\n"
        "def nei(a: int, b: int) -> bool:\n"
        "    return a != b\n"
        "def truthy(x: int) -> int:\n"
        "    if x:\n"
        "        return 111\n"
        "    return 222\n"
        "def boolx(x: int) -> bool:\n"
        "    return bool(x)\n"
        "\n"
        "BIG = 1 << 60\n"
        "ZLOW = 1 << 47\n"  # BigInt whose low 47 bits are all zero
        "print(addi(BIG, 7))\n"
        "print(subi(BIG, 1))\n"
        "print(muli(BIG, 2))\n"
        "print(muli(BIG, BIG))\n"
        "print(fdiv(BIG, 2))\n"
        "print(modi(BIG, 1000))\n"
        "print(andi(BIG, BIG))\n"
        "print(ori(BIG, 1))\n"
        "print(xori(BIG, 1))\n"
        "print(lsh(1, 60))\n"
        "print(rsh(BIG, 2))\n"
        "print(negi(BIG), absi(-BIG), inv(BIG))\n"
        "print(negi(ZLOW), absi(-ZLOW), inv(ZLOW))\n"
        "print(lti(BIG, BIG + 1), lti(5, BIG))\n"
        "print(eqi(BIG, BIG), eqi(BIG, BIG + 1))\n"
        "print(nei(BIG, BIG), nei(BIG, BIG + 1))\n"
        "print(eqi(True, 1), nei(True, 0))\n"  # bool/int value equality
        "print(truthy(BIG), truthy(ZLOW), truthy(0), truthy(False))\n"
        "print(boolx(BIG), boolx(ZLOW), boolx(0), boolx(False))\n",
        encoding="utf-8",
    )

    rc, out, err = _run_native(root, src)
    assert rc == 0, out + err
    assert out.strip().splitlines() == [
        "1152921504606846983",
        "1152921504606846975",
        "2305843009213693952",
        "1329227995784915872903807060280344576",
        "576460752303423488",
        "976",
        "1152921504606846976",
        "1152921504606846977",
        "1152921504606846977",
        "1152921504606846976",
        "288230376151711744",
        "-1152921504606846976 1152921504606846976 -1152921504606846977",
        "-140737488355328 140737488355328 -140737488355329",
        "True True",
        "True False",
        "False True",
        "True True",
        "111 111 222 222",
        "True True False False",
    ]


def test_native_bigint_true_division_is_correctly_rounded(tmp_path: Path) -> None:
    """``int / int`` true division of BigInt operands must produce the same
    correctly-rounded IEEE-754 double as CPython (compared via ``float.hex`` to be
    independent of ``repr`` formatting), instead of raising ``OverflowError`` or
    truncating."""
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bigint_truediv.py"
    src.write_text(
        "def divi(a: int, b: int) -> float:\n"
        "    return a / b\n"
        "\n"
        "BIG = 1 << 60\n"
        "print(float.hex(divi(BIG, 2)))\n"
        "print(float.hex(divi(BIG, 3)))\n"
        "print(float.hex(divi(10 ** 30, 7)))\n"
        "print(float.hex(divi(-(1 << 70), 3)))\n"
        "print(float.hex(divi(2 ** 100, 2 ** 50)))\n",
        encoding="utf-8",
    )

    rc, out, err = _run_native(root, src)
    assert rc == 0, out + err
    assert out.strip().splitlines() == [
        float.hex((1 << 60) / 2),
        float.hex((1 << 60) / 3),
        float.hex((10**30) / 7),
        float.hex((-(1 << 70)) / 3),
        float.hex((2**100) / (2**50)),
    ]

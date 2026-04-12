from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from molt.frontend import compile_to_tir


ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"
SESSION_ID = "pytest-gpu-kernel-compiled"


def _native_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env["MOLT_SESSION_ID"] = SESSION_ID
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(ROOT / "target")
    env["MOLT_CACHE"] = str(ROOT / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(ROOT / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(ROOT / "tmp")
    env["UV_CACHE_DIR"] = str(ROOT / ".uv-cache")
    env["TMPDIR"] = str(ROOT / "tmp")
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def test_compiled_gpu_kernel_vector_add_matches_interpreted_semantics(
    tmp_path: Path,
) -> None:
    src_path = tmp_path / "gpu_kernel_smoke.py"
    out_path = tmp_path / "gpu_kernel_smoke"
    src_path.write_text(
        "import molt.gpu as gpu\n"
        "\n"
        "@gpu.kernel\n"
        "def vector_add(a, b, c, n):\n"
        "    tid = gpu.thread_id()\n"
        "    if tid < n:\n"
        "        c[tid] = a[tid] + b[tid]\n"
        "\n"
        "a = gpu.to_device([1.0, 2.0, 3.0, 4.0])\n"
        "b = gpu.to_device([10.0, 20.0, 30.0, 40.0])\n"
        "c = gpu.alloc(4, float)\n"
        "vector_add[1, 4](a, b, c, 4)\n"
        "print(gpu.from_device(c))\n",
        encoding="utf-8",
    )

    env = _native_env()
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--backend",
            "cranelift",
            "--output",
            str(out_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert build.returncode == 0, build.stdout + build.stderr

    run = subprocess.run(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[11.0, 22.0, 33.0, 44.0]"


def test_gpu_kernel_call_lowers_to_first_class_gpu_launch_ir(tmp_path: Path) -> None:
    ir = compile_to_tir(
        "import molt.gpu as gpu\n"
        "\n"
        "@gpu.kernel\n"
        "def vector_add(a, b, c, n):\n"
        "    tid = gpu.thread_id()\n"
        "    if tid < n:\n"
        "        c[tid] = a[tid] + b[tid]\n"
        "\n"
        "a = gpu.to_device([1.0, 2.0, 3.0, 4.0])\n"
        "b = gpu.to_device([10.0, 20.0, 30.0, 40.0])\n"
        "c = gpu.alloc(4, float)\n"
        "vector_add[1, 4](a, b, c, 4)\n"
    )
    module_main = next(
        func for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "molt_gpu_kernel_launch"
        for op in module_main["ops"]
    )


def test_gpu_kernel_descriptor_is_attached_to_function_metadata() -> None:
    ir = compile_to_tir(
        "import molt.gpu as gpu\n"
        "\n"
        "@gpu.kernel\n"
        "def vector_add(a, b, c, n):\n"
        "    tid = gpu.thread_id()\n"
        "    if tid < n:\n"
        "        c[tid] = a[tid] + b[tid]\n"
        "\n"
        "a = gpu.to_device([1.0, 2.0, 3.0, 4.0])\n"
        "b = gpu.to_device([10.0, 20.0, 30.0, 40.0])\n"
        "c = gpu.alloc(4, float)\n"
        "vector_add[1, 4](a, b, c, 4)\n"
    )

    descriptor_set = False
    descriptor_payload = None
    for func in ir["functions"]:
        for index, op in enumerate(func["ops"]):
            if (
                op.get("kind") == "set_attr_generic_obj"
                and op.get("s_value") == "__molt_gpu_descriptor__"
            ):
                descriptor_set = True
                value_name = op["args"][1]
                for prior in reversed(func["ops"][:index]):
                    if prior.get("out") == value_name and prior.get("kind") == "const_str":
                        descriptor_payload = prior.get("s_value")
                        break
    assert descriptor_set is True
    assert isinstance(descriptor_payload, str)
    assert '"kind":"molt_gpu_kernel"' in descriptor_payload
    assert '"name":"vector_add"' in descriptor_payload
    assert '"symbol":"__main____vector_add"' in descriptor_payload

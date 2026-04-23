"""
Modal GPU deployment for Nemotron OCR v2.

Provides high-throughput batch OCR for API agents.
Throughput claims must be measured per deployment; upstream NVIDIA material
reports high A100 throughput, but this wrapper treats local measurements as
the production source of truth.

Usage:
    modal run deploy/modal/nemotron_ocr.py        # smoke test
    modal deploy deploy/modal/nemotron_ocr.py     # persistent deployment

Endpoint:
    POST /ocr  {"image_base64": "...", "lang": "en"|"multi", "merge_level": "word"|"sentence"|"paragraph"}
"""

from contextlib import ExitStack, contextmanager
import base64
import binascii
import io
import os
import tempfile
import time

import modal

app = modal.App("nemotron-ocr")

LANG_ALIASES = {
    "en": "en",
    "english": "en",
    "multi": "multi",
    "multilingual": "multi",
}
SUPPORTED_MERGE_LEVELS = {"word", "sentence", "paragraph"}


def _normalize_lang(lang: str) -> str:
    if not isinstance(lang, str):
        raise ValueError("Unsupported lang: expected 'en' or 'multi'")
    normalized = LANG_ALIASES.get(lang.strip().lower())
    if normalized is None:
        raise ValueError(
            f"Unsupported lang: expected one of {', '.join(sorted(LANG_ALIASES))}"
        )
    return normalized


def _normalize_merge_level(merge_level: str) -> str:
    if not isinstance(merge_level, str):
        raise ValueError(
            "Unsupported merge_level: expected 'word', 'sentence', or 'paragraph'"
        )
    normalized = merge_level.strip().lower()
    if normalized not in SUPPORTED_MERGE_LEVELS:
        raise ValueError(
            "Unsupported merge_level: expected one of "
            f"{', '.join(sorted(SUPPORTED_MERGE_LEVELS))}"
        )
    return normalized


@contextmanager
def _temporary_rgb_png_from_base64(image_base64: str):
    """Decode a base64 image into a temp PNG path and always remove it."""
    from PIL import Image

    if not isinstance(image_base64, str) or not image_base64:
        raise ValueError("image_base64 is required")
    try:
        raw = base64.b64decode(image_base64, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise ValueError("image_base64 must be valid base64") from exc

    image = Image.open(io.BytesIO(raw)).convert("RGB")
    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(
            suffix=".png",
            delete=False,
            dir=os.environ.get("TMPDIR"),
        ) as tmp:
            image.save(tmp, format="PNG")
            tmp_path = tmp.name
        yield tmp_path
    finally:
        if tmp_path is not None:
            try:
                os.unlink(tmp_path)
            except FileNotFoundError:
                pass


def _format_region(pred: dict) -> dict:
    return {
        "text": pred["text"],
        "confidence": round(float(pred["confidence"]), 4),
        "bbox": [
            float(pred["left"]),
            float(pred["upper"]),
            float(pred["right"]),
            float(pred["upper"]),
            float(pred["right"]),
            float(pred["lower"]),
            float(pred["left"]),
            float(pred["lower"]),
        ],
    }


def _format_ocr_result(predictions, latency_ms: float | None = None) -> dict:
    regions = [_format_region(pred) for pred in predictions]
    result = {
        "regions": regions,
        "full_text": "\n".join(region["text"] for region in regions),
    }
    if latency_ms is not None:
        result["latency_ms"] = round(latency_ms, 1)
    return result


nemotron_image = (
    modal.Image.from_registry(
        # Use NVIDIA's PyTorch container WITHOUT add_python override.
        # The CUDA C++ extension (nemotron_ocr_cpp) must compile against
        # the container's native Python + CUDA toolkit. Injecting a separate
        # Python 3.12 breaks the CUDA header paths.
        "nvcr.io/nvidia/pytorch:24.12-py3",
    )
    .run_commands(
        # Install git-lfs and build dependencies
        "apt-get update && apt-get install -y git-lfs && git lfs install",
        "pip install hatchling httpx pillow",
        # Clone and build nemotron-ocr (CUDA C++ extension + Python package)
        "git clone https://huggingface.co/nvidia/nemotron-ocr-v2 /models/nemotron-ocr-v2",
        "cd /models/nemotron-ocr-v2/nemotron-ocr && pip install --no-build-isolation .",
    )
)


@app.cls(
    image=nemotron_image,
    gpu="A10G",  # 24 GB VRAM — English variant uses ~1.5 GB, multilingual ~2.5 GB
    timeout=120,
    scaledown_window=300,
)
@modal.concurrent(max_inputs=32)
class NemotronOCR:
    """High-throughput Nemotron OCR v2 inference."""

    @modal.enter()
    def load_model(self) -> None:
        """Load model once per container lifetime."""
        from nemotron_ocr.inference.pipeline_v2 import NemotronOCRV2

        # Load both variants — English for speed, multilingual for coverage
        self.ocr_en = NemotronOCRV2(model_dir="/models/nemotron-ocr-v2/v2_english")
        self.ocr_multi = NemotronOCRV2(
            model_dir="/models/nemotron-ocr-v2/v2_multilingual"
        )

    @modal.method()
    async def ocr(
        self,
        image_base64: str,
        lang: str = "en",
        merge_level: str = "word",
    ) -> dict:
        """
        Run Nemotron OCR v2 on a single image.

        Args:
            image_base64: Base64-encoded PNG or JPEG.
            lang: "en" for English-optimized, "multi" for multilingual.
            merge_level: "word", "sentence", or "paragraph".

        Returns:
            {
                "regions": [{"text": str, "confidence": float, "bbox": [x1,y1,x2,y2,x3,y3,x4,y4]}],
                "full_text": str,
                "latency_ms": float,
            }
        """
        start = time.perf_counter()
        normalized_lang = _normalize_lang(lang)
        normalized_merge_level = _normalize_merge_level(merge_level)

        model = self.ocr_en if normalized_lang == "en" else self.ocr_multi

        with _temporary_rgb_png_from_base64(image_base64) as tmp_path:
            predictions = model(tmp_path, merge_level=normalized_merge_level)

        return _format_ocr_result(
            predictions,
            latency_ms=(time.perf_counter() - start) * 1000,
        )

    @modal.method()
    async def ocr_batch(
        self,
        images_base64: list[str],
        lang: str = "en",
        merge_level: str = "word",
    ) -> list[dict]:
        """
        Batch OCR for multiple images using NemotronOCRV2's native batch API.
        """
        if not isinstance(images_base64, list) or not images_base64:
            raise ValueError("images_base64 must be a non-empty list")

        start = time.perf_counter()
        normalized_lang = _normalize_lang(lang)
        normalized_merge_level = _normalize_merge_level(merge_level)
        model = self.ocr_en if normalized_lang == "en" else self.ocr_multi

        with ExitStack() as stack:
            image_paths = [
                stack.enter_context(_temporary_rgb_png_from_base64(image_base64))
                for image_base64 in images_base64
            ]
            batch_predictions = model(
                image_paths,
                merge_level=normalized_merge_level,
            )
            if len(batch_predictions) != len(image_paths):
                raise RuntimeError(
                    "Nemotron OCR batch contract violated: expected one "
                    "prediction list per input image"
                )
            latency_per_image_ms = (
                (time.perf_counter() - start) * 1000 / len(image_paths)
            )
            return [
                _format_ocr_result(predictions, latency_ms=latency_per_image_ms)
                for predictions in batch_predictions
            ]


@app.function(
    image=nemotron_image,
    gpu="A10G",
    timeout=120,
    scaledown_window=300,
)
@modal.fastapi_endpoint(method="POST", docs=True)
async def ocr_endpoint(item: dict) -> dict:
    """
    HTTP POST endpoint for Nemotron OCR.

    Body: {"image_base64": "...", "lang": "en", "merge_level": "word"}
    """
    image_base64 = item.get("image_base64", "")
    lang = item.get("lang", "en")
    merge_level = item.get("merge_level", "word")

    if not image_base64:
        return {"error": "image_base64 is required"}

    srv = NemotronOCR()
    return await srv.ocr.remote(image_base64, lang, merge_level)


@app.local_entrypoint()
def main():
    """Smoke test with a minimal image."""
    import base64
    import io

    from PIL import Image

    img = Image.new("RGB", (64, 64), (255, 255, 255))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    test_b64 = base64.b64encode(buf.getvalue()).decode()

    srv = NemotronOCR()
    result = srv.ocr.remote(test_b64, "en", "word")
    print(f"Result: {result}")

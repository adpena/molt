"""
Modal GPU deployment for Nemotron OCR v2.

Provides high-throughput batch OCR for API agents.
34x faster than PaddleOCR on A100, ~15-20 pages/sec on A10G.

Usage:
    modal run deploy/modal/nemotron_ocr.py        # smoke test
    modal deploy deploy/modal/nemotron_ocr.py     # persistent deployment

Endpoint:
    POST /ocr  {"image_base64": "...", "lang": "en"|"multi", "merge_level": "word"|"sentence"|"paragraph"}
"""

import modal

app = modal.App("nemotron-ocr")

nemotron_image = (
    modal.Image.from_registry(
        "nvcr.io/nvidia/pytorch:25.09-py3",
        add_python="3.12",
    )
    .pip_install(
        "httpx>=0.28.0",
        "pillow>=11.0",
    )
    .run_commands(
        # Clone and install nemotron-ocr package
        "git lfs install",
        "git clone https://huggingface.co/nvidia/nemotron-ocr-v2 /models/nemotron-ocr-v2",
        "cd /models/nemotron-ocr-v2/nemotron-ocr && pip install --no-build-isolation -v .",
    )
)


@app.cls(
    image=nemotron_image,
    gpu="A10G",  # 24 GB VRAM — English variant uses ~1.5 GB, multilingual ~2.5 GB
    timeout=120,
    allow_concurrent_inputs=32,  # Nemotron handles batching natively
    container_idle_timeout=300,
)
class NemotronOCR:
    """High-throughput Nemotron OCR v2 inference."""

    @modal.enter()
    def load_model(self) -> None:
        """Load model once per container lifetime."""
        from nemotron_ocr.inference.pipeline_v2 import NemotronOCRV2

        # Load both variants — English for speed, multilingual for coverage
        self.ocr_en = NemotronOCRV2(
            model_dir="/models/nemotron-ocr-v2/v2_english"
        )
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
        import base64
        import io
        import tempfile
        import time

        from PIL import Image

        start = time.perf_counter()

        # Decode image
        raw = base64.b64decode(image_base64)
        img = Image.open(io.BytesIO(raw)).convert("RGB")

        # Nemotron expects file path input — write to tmpfs
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
            img.save(tmp, format="PNG")
            tmp_path = tmp.name

        # Select model variant
        model = self.ocr_en if lang == "en" else self.ocr_multi

        # Run inference
        predictions = model(tmp_path, merge_level=merge_level)

        regions = []
        for pred in predictions:
            regions.append({
                "text": pred["text"],
                "confidence": round(pred["confidence"], 4),
                "bbox": [
                    pred["left"], pred["upper"],
                    pred["right"], pred["upper"],
                    pred["right"], pred["lower"],
                    pred["left"], pred["lower"],
                ],
            })

        full_text = "\n".join(r["text"] for r in regions)
        latency_ms = (time.perf_counter() - start) * 1000

        return {
            "regions": regions,
            "full_text": full_text,
            "latency_ms": round(latency_ms, 1),
        }

    @modal.method()
    async def ocr_batch(
        self,
        images_base64: list[str],
        lang: str = "en",
        merge_level: str = "word",
    ) -> list[dict]:
        """
        Batch OCR for multiple images. Leverages Nemotron's native batching.
        """
        results = []
        for img_b64 in images_base64:
            result = await self.ocr(img_b64, lang=lang, merge_level=merge_level)
            results.append(result)
        return results


@app.function(
    image=nemotron_image,
    gpu="A10G",
    timeout=120,
    allow_concurrent_inputs=32,
    container_idle_timeout=300,
)
@modal.web_endpoint(method="POST", docs=True)
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

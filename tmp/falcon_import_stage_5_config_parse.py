from pathlib import Path

from main_molt import FalconOCRConfig

cfg = FalconOCRConfig.from_json(
    Path(
        "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/config.json"
    ).read_text()
)
print(cfg.dim, cfg.n_layers, cfg.img_start_id)

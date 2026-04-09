print("stage: start")

from pathlib import Path

print("stage: pathlib")

from main_molt import init

print("stage: imported")

config_json = Path(
    "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/config.json"
).read_text()

print("stage: config")

init(
    "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/model.safetensors",
    config_json,
)

print("stage: init")

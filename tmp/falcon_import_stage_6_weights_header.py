import json
import struct

with open(
    "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/model.safetensors",
    "rb",
) as f:
    header_len = struct.unpack("<Q", f.read(8))[0]
    header = json.loads(f.read(header_len).decode("utf-8"))

keys = [name for name in header if name != "__metadata__"]
print(len(keys), keys[0], header[keys[0]]["shape"])

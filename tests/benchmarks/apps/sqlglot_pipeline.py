"""TPC-H SQL parse, optimization, AST serialization and Spark SQL emission."""

import json
from pathlib import Path
import sys

from sqlglot import parse_one
from sqlglot.optimizer import optimize


def main():
    fixture = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    repetitions = int(sys.argv[2])
    if repetitions < 1:
        raise ValueError("repetitions must be positive")
    result = None
    for _ in range(repetitions):
        expression = parse_one(fixture["SQL"])
        optimized = optimize(expression.copy(), schema=fixture["TPCH_SCHEMA"])
        result = {
            "parsed": expression.dump(),
            "optimized": optimized.dump(),
            "spark": optimized.sql(dialect="spark"),
        }
    print(json.dumps(result, ensure_ascii=True, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()

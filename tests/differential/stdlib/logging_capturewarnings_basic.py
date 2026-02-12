"""Purpose: differential coverage for logging.captureWarnings."""

import io
import logging
import warnings


def main():
    stream = io.StringIO()
    handler = logging.StreamHandler(stream)
    logger = logging.getLogger("py.warnings")
    logger.handlers[:] = [handler]
    logger.setLevel(logging.WARNING)

    logging.captureWarnings(True)
    warnings.warn("warn-one", UserWarning)
    logging.captureWarnings(False)

    handler.flush()
    output = stream.getvalue()
    print("captured", "warn-one" in output)


if __name__ == "__main__":
    main()

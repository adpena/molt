"""Purpose: differential coverage for logging.disable levels."""

import io
import logging


def main():
    stream = io.StringIO()
    handler = logging.StreamHandler(stream)
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(logging.DEBUG)

    logging.disable(logging.ERROR)
    root.error("blocked")
    logging.disable(logging.NOTSET)
    root.error("shown")

    handler.flush()
    output = stream.getvalue()
    print("blocked", "blocked" in output)
    print("shown", "shown" in output)


if __name__ == "__main__":
    main()

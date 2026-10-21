"""Purpose: module source paths keep CPython's shape, and the stdlib never names
the compiler checkout that built the program (HF-372)."""

import json
import json.decoder
import os
import sys
import traceback


def tail(path, count):
    return "/".join(path.replace("\\", "/").split("/")[-count:])


def program_function():
    return program_function.__code__.co_filename


# A program module reports the absolute file it was compiled from.
print("main_file_basename", os.path.basename(__file__))
print("main_file_is_absolute", os.path.isabs(__file__))
print("main_code_names_main_file", program_function() == __file__)

# A stdlib module keeps its package-relative shape and one path for every view.
print("json_file_tail", tail(json.__file__, 2))
print("json_path_is_file_directory", json.__path__ == [os.path.dirname(json.__file__)])
print("json_origin_is_file", json.__spec__.origin == json.__file__)
print("decoder_file_tail", tail(json.decoder.__file__, 2))
print("decoder_beside_package_init", os.path.dirname(json.decoder.__file__) == json.__path__[0])
print(
    "decoder_code_names_decoder_file",
    json.decoder.py_scanstring.__code__.co_filename == json.decoder.__file__,
)
# CPython reports its installed stdlib. A compiled program must not report the
# checkout or worktree that compiled it.
print("stdlib_names_no_compiler_checkout", "/src/molt/" not in json.__file__.replace("\\", "/"))


def hook(pairs):
    raise ValueError("hook")


def hook_frames():
    try:
        json.loads('{"key": 1}', object_hook=hook)
    except ValueError as exc:
        return traceback.extract_tb(exc.__traceback__)


# A traceback through the stdlib names each frame's module file. (Native
# program source lines in tracebacks: tests/cli/test_traceback_carets.py.)
frames = hook_frames()
module_files = {getattr(module, "__file__", None) for module in list(sys.modules.values())}
raising = [frame for frame in frames if frame.name == "hook"]
print("hook_frame_count", len(raising))
print("hook_frame_is_this_file", raising[-1].filename == __file__)
print("frames_name_module_files", all(frame.filename in module_files for frame in frames))
print("a_frame_names_json_file", any(frame.filename == json.__file__ for frame in frames))

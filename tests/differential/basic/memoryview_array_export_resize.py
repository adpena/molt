"""Purpose: memoryview(array.array) exports a live typed buffer and pins resizes."""

import array
import io


arr = array.array("h", [1, 2, 3])
view = memoryview(arr)
tail = view[1:]
stride = view[::2]
nested = view[1:][1:]
empty_stride_tail = view[1::2][1:]

print(view.format, view.itemsize, view.shape, view.strides, view.tolist())
print(tail.format, tail.tolist())
print(stride.format, stride.shape, stride.strides, stride.tolist())
print(empty_stride_tail.format, empty_stride_tail.shape, empty_stride_tail.strides, empty_stride_tail.tolist())

tail[:] = memoryview(array.array("h", [9, 8]))
print(arr.tolist())

nested[:] = memoryview(array.array("h", [7]))
print(nested.format, nested.shape, nested.strides, nested.tolist())
print(arr.tolist())

try:
    arr.append(4)
except Exception as exc:
    print(type(exc).__name__, str(exc))

view.release()
try:
    arr.append(4)
except Exception as exc:
    print(type(exc).__name__, str(exc))

stride.release()
nested.release()
empty_stride_tail.release()
tail.release()
arr.append(4)
print(arr.tolist())

cast_arr = array.array("h", [11, 22, 33])
cast_root = memoryview(cast_arr)
cast_bytes = cast_root.cast("B")
cast_tail = cast_bytes[cast_root.itemsize :].cast("h")
print(cast_tail.format, cast_tail.tolist())
cast_root.release()
cast_bytes.release()
try:
    cast_arr.append(44)
except Exception as exc:
    print("cast pin", type(exc).__name__, str(exc))
cast_tail.release()
cast_arr.append(44)
print(cast_arr.tolist())

buf = bytearray(b"abc")
buf_view = memoryview(buf)
try:
    buf.append(120)
except Exception as exc:
    print("bytearray append", type(exc).__name__)

buf[0] = 122
print(buf_view.tolist())

try:
    del buf[0]
except Exception as exc:
    print("bytearray del", type(exc).__name__)

try:
    buf += b"x"
except Exception as exc:
    print("bytearray iadd", type(exc).__name__)

try:
    buf *= 0
except Exception as exc:
    print("bytearray imul zero", type(exc).__name__)

try:
    buf *= 2
except Exception as exc:
    print("bytearray imul grow", type(exc).__name__)

buf_view.release()
buf += b"x"
buf *= 2
buf.append(120)
print(memoryview(buf).tolist())

bio = io.BytesIO(b"abc")
bio_view = bio.getbuffer()
try:
    bio.seek(0)
    bio.write(b"Z")
except Exception as exc:
    print("bytesio write same", type(exc).__name__)

try:
    bio.seek(0)
    bio.write(b"")
except Exception as exc:
    print("bytesio write empty", type(exc).__name__)

try:
    bio.writelines([b""])
except Exception as exc:
    print("bytesio writelines empty", type(exc).__name__)

try:
    bio.seek(3)
    bio.write(b"x")
except Exception as exc:
    print("bytesio write", type(exc).__name__)

try:
    bio.truncate(3)
except Exception as exc:
    print("bytesio truncate same", type(exc).__name__)

try:
    bio.truncate(1)
except Exception as exc:
    print("bytesio truncate small", type(exc).__name__)

try:
    bio.truncate(5)
except Exception as exc:
    print("bytesio truncate grow", type(exc).__name__)

bio_view[0] = 121
print(memoryview(bio.getvalue()).tolist())
bio_view.release()
bio.seek(3)
print(bio.write(b"x"), bio.getvalue())
print(bio.truncate(2), bio.getvalue())

bio_close = io.BytesIO(b"abc")
bio_close_view = bio_close.getbuffer()
try:
    bio_close.close()
except Exception as exc:
    print("bytesio close", type(exc).__name__, bio_close.closed)
bio_close_view.release()
bio_close.close()
print("bytesio closed", bio_close.closed)

bio_exit = io.BytesIO(b"abc")
bio_exit_view = bio_exit.getbuffer()
try:
    bio_exit.__exit__(None, None, None)
except Exception as exc:
    print("bytesio exit", type(exc).__name__, bio_exit.closed)
bio_exit_view.release()
bio_exit.close()
print("bytesio exit closed", bio_exit.closed)

bio_text = io.BytesIO(b"abc")
bio_text_view = bio_text.getbuffer()
text = io.TextIOWrapper(bio_text, encoding="utf-8")
try:
    text.close()
except Exception as exc:
    print("text bytesio close", type(exc).__name__, text.closed, bio_text.closed)
bio_text_view.release()
text.close()
print("text bytesio closed", text.closed, bio_text.closed)

bio_buffered = io.BytesIO(b"abc")
bio_buffered_view = bio_buffered.getbuffer()
writer = io.BufferedWriter(bio_buffered, buffer_size=8)
try:
    writer.write(b"Z")
    writer.flush()
except Exception as exc:
    print("buffered bytesio flush", type(exc).__name__, bio_buffered.getvalue())
bio_buffered_view.release()
writer.flush()
print("buffered bytesio flushed", bio_buffered.getvalue())
writer.close()

# MOLT_META: stderr=exact
"""Exercise native owners missing from existing file/stream close fixtures."""

import bz2
import codecs
import dbm.dumb
from html.parser import HTMLParser
import io
import hashlib
import http.client
import lzma
import os
import shutil
import sqlite3
import tempfile
import zlib


# Keep the façade owners alive through module teardown, including a cursor
# whose logical close does not release Molt's native cursor registry entry.
connection = sqlite3.connect(":memory:")
cursor = connection.cursor()
cursor.execute("select 42")
print("sqlite", cursor.fetchone() == (42,))
cursor.close()
cursor.close()


class Parser(HTMLParser):
    def handle_data(self, data):
        self.payload = data


parser = Parser()
parser.feed("<p>payload</p>")
parser.close()
print("html", parser.payload == "payload")

reader = codecs.getreader("utf-8")(io.BytesIO(b"payload"))
print("reader", reader.read() == "payload")

encoder = codecs.getincrementalencoder("utf-8")()
decoder = codecs.getincrementaldecoder("utf-8")()
print("codecs", decoder.decode(encoder.encode("payload", True), True) == "payload")
hash_owner = hashlib.sha256(b"payload")
print("hash", len(hash_owner.digest()) == 32)
zcompressor = zlib.compressobj()
zdecompressor = zlib.decompressobj()
zpayload = zcompressor.compress(b"payload") + zcompressor.flush()
print("zlib", zdecompressor.decompress(zpayload) == b"payload")

headers = http.client.HTTPMessage()
headers["X-Owner"] = "retained"
print("headers", headers["X-Owner"] == "retained")

owners = [
    connection,
    cursor,
    parser,
    reader,
    encoder,
    decoder,
    hash_owner,
    zcompressor,
    zdecompressor,
    headers,
]
directory = tempfile.mkdtemp()
try:
    for file_type, name in ((bz2.BZ2File, "data.bz2"), (lzma.LZMAFile, "data.xz")):
        path = os.path.join(directory, name)
        writer = file_type(path, "wb")
        writer.write(b"payload")
        writer.close()
        writer.close()
        with file_type(path, "rb") as compressed_reader:
            print(name, compressed_reader.read() == b"payload")
        owners.extend((writer, compressed_reader))
        try:
            writer.write(b"after close")
        except ValueError:
            print(name, "closed")

    db_path = os.path.join(directory, "store")
    database = dbm.dumb.open(db_path, "c")
    database[b"key"] = b"value"
    database.close()
    database.close()
    readonly_database = dbm.dumb.open(db_path, "r")
    print("dbm", readonly_database[b"key"] == b"value")
    owners.extend((database, readonly_database))
finally:
    # Readonly dbm keeps its in-memory owner but performs no write on close.
    # Compression file owners above are already closed on every successful path.
    shutil.rmtree(directory)

# MOLT_META: stderr=exact
"""Independent bounded incremental-decoder state and retained-input oracle.

Compressed literals were produced once with CPython 3.12.15: bz2 level 1,
LZMA XZ/ALONE preset 0. Runtime expectations are the literal payload, never a
Molt compressor round trip. The XZ fixture needs more than a one-byte memlimit.
"""

import bz2
import lzma
import sys

PAYLOAD = b"codec-stream-state\n" + bytes(range(256)) * 4 + b"\x00TAIL\n"
SECOND_PAYLOAD = b"second-member\n" + bytes(range(32))

BZ2_MEMBER = bytes.fromhex(
    "425a683131415926535959aebed50000ce7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    "ffffffc0021c000126000980009800000000000000000000000000000000000000000000000000000000930004c0004c"
    "000000000000000000000000000000000000000000000000000000004980026000260000000000000000000000000000"
    "000000000000000000000000000005555268d34686898269a64c460001313004cd1a26689869a0d2698680680002604c"
    "000230232600989826264c00230002326000000035212388123e480822424192224642119252224c429272507c04a495"
    "12b2584b4f84971f112f3e32604c4f90991332687ca7cc4d49b1372704e49d13b2784f49f13f28046c650ca21f394528"
    "c51cfa0a414929452ca614d29c53ca81f49512a4431155421caa9563ea2ae5608820eb25688f7d64315b2b8574af1125"
    "7c898a256580b09622c6459643ec2ca7da598b39682c5778c206d445dce94486ed6d804196e2de5c08d2e25c885b990c"
    "49ae849cba928bb10e949779247a01045e0bc97a2f65f0be97e2fe600fb8c0982306610c29863ef30e620c498a316630"
    "c698e31e640c8992326650ca9963f03f132e660cc99a336670fc8fccce99e3f433e680d09a2346690d29a634e7ea6a0d"
    "49aa3f63566b0d69ae35e7ee6c0d89b23f83666d0fe4da9b636e6e0dc9ba3766f0de9be37e700fe8fece09c2386710e2"
    "9c638e720e49ca396730e69ce3fc3fd39e740ff8e89d23a6750ea9d63ae760ec9da3b6770ee9de3be780f09e23c6790f"
    "29e63ce7a0f49ea3d67b0f69ee3de471ff8bb9229c28482cd75f6a80"
)

BZ2_SECOND = bytes.fromhex(
    "425a6831314159265359c7819f3e000016f1807fffffff80021e03980020002307a41a68c83400d0d19a6a186864c819"
    "188313268698aa4940d34f452782115d8955b75f82f1cb3d35db7e39675f3de2402612202fc5dc914e142431e067cf80"
)

XZ_MEMBER = bytes.fromhex(
    "fd377a585a000004e6d6b446020021010c0000008f98419ce0041801085d00319bc8b0567f59bfde125ea5e4a89ff47c"
    "bf6edfd1d37e270e5238ddfd9246b9a4fae6a6dcc27250e0543d0ac5452549c934efb95e3ddd11421a82de9d89379b40"
    "38349c1be57d40c9ea859570fef3020c6bec83dee7a5bfd4afab3c6e434572f797470fcd7f391c944b09332c9d293131"
    "b1094be00125d7b76866b818ca21ade1b8b6fad33849a23c61aee6a7b88a1df8908f877a103dbc10e4f618190f3840ee"
    "e76c1abd1f971d7792157c03c13d1551b7cae77f39251b42197a52dec76550123ab6897f7facf4f1a79ea499c4e5dc3f"
    "8950f9ba74ea155531df5130596ca139165c8e66de230c2025423af6429fd1924da341591033470fa9555bb6afea427f"
    "97bb17448c4800000e309d4f176ed2020001a40299080000d32b43f5b1c467fb020000000004595a"
)

XZ_SECOND = bytes.fromhex(
    "fd377a585a000004e6d6b446020021010c0000008f98419c01002d7365636f6e642d6d656d6265720a00010203040506"
    "0708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f000000ff912d248a961d0a0001462e6750735a1fb6f37d"
    "010000000004595a"
)

ALONE_MEMBER = bytes.fromhex(
    "5d00000400ffffffffffffffff00319bc8b0567f59bfde125ea5e4a89ff47cbf6edfd1d37e270e5238ddfd9246b9a4fa"
    "e6a6dcc27250e0543d0ac5452549c934efb95e3ddd11421a82de9d89379b4038349c1be57d40c9ea859570fef3020c6b"
    "ec83dee7a5bfd4afab3c6e434572f797470fcd7f391c944b09332c9d293131b1094be00125d7b76866b818ca21ade1b8"
    "b6fad33849a23c61aee6a7b88a1df8908f877a103dbc10e4f618190f3840eee76c1abd1f971d7792157c03c13d1551b7"
    "cae77f39251b42197a52dec76550123ab6897f7facf4f1a79ea499c4e5dc3f8950f9ba74ea155531df5130596ca13916"
    "5c8e66de230c2025423af6429fd1924da341591033470fa9555bb6afea427f97bb174a77a9f7ffffb8bc6a00"
)

ALONE_SECOND = bytes.fromhex(
    "5d00000400ffffffffffffffff0039994891b169965af63f9626254c881b44be1c49b24da210f88fbbcb6f476544fb09"
    "ab5a9bfe6f5195f2990e0674a46effffb3ae0000"
)


def new_bz2():
    return bz2.BZ2Decompressor()


def new_xz():
    return lzma.LZMADecompressor(format=lzma.FORMAT_XZ)


def new_auto():
    return lzma.LZMADecompressor(format=lzma.FORMAT_AUTO)


def new_alone():
    return lzma.LZMADecompressor(format=lzma.FORMAT_ALONE)


def decode_bounded(label, factory, member, second, chunk_size, limit):
    decoder = factory()
    encoded = member + second
    cursor = 0
    output = bytearray()
    empty_drains = 0
    zero_calls = 0
    # Even one-byte input/output must finish in this finite byte-based budget.
    budget = 3 * (len(encoded) + len(PAYLOAD)) + 32
    for _ in range(budget):
        if decoder.eof:
            break
        if decoder.needs_input:
            if cursor == len(encoded):
                raise AssertionError((label, "input exhausted before EOF"))
            block = encoded[cursor : cursor + chunk_size]
            cursor += len(block)
        else:
            block = b""
            empty_drains += 1
        # A zero-output call retains its input; drain it with empty input and a
        # positive bound rather than feeding the same compressed bytes twice.
        bound = 31 if limit == 0 and not block else limit
        part = decoder.decompress(block, max_length=bound)
        if not isinstance(part, bytes) or len(part) > bound:
            raise AssertionError((label, "output bound", len(part), bound))
        if bound == 0:
            zero_calls += 1
            if part:
                raise AssertionError((label, "zero limit produced output"))
        output.extend(part)
        if len(output) > len(PAYLOAD):
            raise AssertionError((label, "replayed output"))
        if not block and not part and not decoder.needs_input and not decoder.eof:
            raise AssertionError((label, "empty-input drain made no progress"))
    else:
        raise AssertionError((label, "decoder exceeded finite progress budget"))
    retained = decoder.unused_data + encoded[cursor:]
    exact = bytes(output) == PAYLOAD
    tail_exact = retained == second
    finished = decoder.eof and not decoder.needs_input
    if not exact or not tail_exact or not finished:
        raise AssertionError((label, "state loss", exact, tail_exact, finished))
    if limit == 0 and zero_calls == 0:
        raise AssertionError((label, "zero limit unexercised"))
    if chunk_size == len(encoded) and limit in (0, 1, 31) and empty_drains < 2:
        raise AssertionError((label, "repeated empty-input drain unexercised"))
    try:
        decoder.decompress(b"")
    except EOFError:
        pass
    else:
        raise AssertionError((label, "decompress after EOF did not fail"))
    print(
        "stream",
        label,
        "all" if chunk_size == len(encoded) else chunk_size,
        limit,
        exact,
        tail_exact,
        finished,
    )


CASES = (
    ("bz2", new_bz2, BZ2_MEMBER, BZ2_SECOND),
    ("xz", new_xz, XZ_MEMBER, XZ_SECOND),
    ("auto-xz", new_auto, XZ_MEMBER, XZ_SECOND),
    ("alone", new_alone, ALONE_MEMBER, ALONE_SECOND),
    ("auto-alone", new_auto, ALONE_MEMBER, ALONE_SECOND),
)

for label, factory, member, second in CASES:
    for chunk_size in (1, 7, len(member) + len(second)):
        for limit in (0, 1, 31, 8192):
            decode_bounded(label, factory, member, second, chunk_size, limit)
    # The second compressed member is observable data, not silently decoded or
    # discarded by the first incremental decoder. All its bytes were supplied.
    decoder = factory()
    first = decoder.decompress(member + second)
    retained = decoder.unused_data
    next_decoder = factory()
    next_output = next_decoder.decompress(retained)
    observed = (
        first == PAYLOAD,
        retained == second,
        next_output == SECOND_PAYLOAD,
        next_decoder.eof,
    )
    if observed != (True, True, True, True):
        raise AssertionError((label, "concatenated member", observed))
    print("members", label, *observed)

for label, factory, expected_error in (
    ("bz2", new_bz2, OSError),
    ("xz", new_xz, lzma.LZMAError),
    ("auto", new_auto, lzma.LZMAError),
    ("alone", new_alone, lzma.LZMAError),
):
    try:
        factory().decompress(b"not a compressed stream\xff\x00")
    except expected_error as error:
        exact = type(error) is expected_error
        if not exact:
            raise AssertionError(
                (label, "malformed exception subtype", type(error).__name__)
            )
        print("malformed", label, type(error).__name__, exact)
    else:
        raise AssertionError((label, "malformed input was accepted"))

for label, format_value, member in (
    ("xz", lzma.FORMAT_XZ, XZ_MEMBER),
    ("auto-xz", lzma.FORMAT_AUTO, XZ_MEMBER),
    ("alone", lzma.FORMAT_ALONE, ALONE_MEMBER),
    ("auto-alone", lzma.FORMAT_AUTO, ALONE_MEMBER),
):
    try:
        lzma.LZMADecompressor(format=format_value, memlimit=1).decompress(member)
    except lzma.LZMAError as error:
        exact = type(error) is lzma.LZMAError
        if not exact:
            raise AssertionError((label, "memlimit exception subtype"))
        print("memlimit", label, type(error).__name__, exact)
    else:
        raise AssertionError((label, "memory limit was ignored"))

# Clinic argument conversion precedes EOF and never advances the stream on a
# rejected max_length. This also exercises the target-sized integer authority.

for label, factory, member in (
    ("bz2", new_bz2, BZ2_MEMBER),
    ("xz", new_xz, XZ_MEMBER),
):
    for bad, expected in ((None, TypeError), (sys.maxsize + 1, OverflowError)):
        decoder = factory()
        try:
            decoder.decompress(member, bad)
        except expected:
            pass
        else:
            raise AssertionError((label, "bad max_length accepted"))
        if decoder.eof or decoder.decompress(member) != PAYLOAD:
            raise AssertionError((label, "argument rejection advanced decoder"))
        try:
            decoder.decompress(b"", bad)
        except expected:
            pass
        else:
            raise AssertionError((label, "EOF bypassed argument conversion"))
        print("argument", label, expected.__name__, True)


class LimitIndex:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return 1 << 63


limit = LimitIndex()
try:
    lzma.LZMADecompressor(format=lzma.FORMAT_RAW, memlimit=limit)
except ValueError as error:
    if str(error) != "Cannot specify memory limit with FORMAT_RAW" or limit.calls:
        raise AssertionError(("raw memlimit conversion", str(error), limit.calls))
else:
    raise AssertionError("RAW memlimit accepted")
print("raw-memlimit-order", limit.calls)

for value in (None, 1 << 63, (1 << 64) - 1):
    decoder = lzma.LZMADecompressor(memlimit=value)
    if decoder.decompress(XZ_MEMBER) != PAYLOAD:
        raise AssertionError(("unsigned memlimit", value))
print("unsigned-memlimit", True)

limit = LimitIndex()
try:
    lzma.LZMADecompressor(memlimit=limit)
except TypeError:
    if sys.version_info >= (3, 14) or limit.calls:
        raise AssertionError(("memlimit index conversion", limit.calls))
else:
    if sys.version_info < (3, 14) or limit.calls != 1:
        raise AssertionError(("memlimit index conversion", limit.calls))
print("memlimit-index-policy", True)

for value, expected in (
    (-1, ValueError if sys.version_info >= (3, 14) else OverflowError),
    (1 << 64, OverflowError),
):
    try:
        lzma.LZMADecompressor(memlimit=value)
    except expected:
        print("memlimit-reject", expected.__name__)
    else:
        raise AssertionError(("memlimit accepted", value))

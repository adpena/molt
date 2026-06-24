# CLBG-derived kernel, vendored verbatim for molt's perf corpus (doc 69 / 69A, task S4).
# Upstream: The Computer Language Benchmarks Game
#   https://salsa.debian.org/benchmarksgame-team/benchmarksgame
#   repo_ref 40296663ed350d5fe4a6ab5e367bab61cb77c219  (site 25.03)
#   zip entry: revcomp/revcomp.python3-2.python3
# Revised BSD (3-clause) -- see ./LICENSE. Original contributor credits retained below.
# Per BSD clause 3, do NOT use the CLBG / "Benchmarks Game" names to promote molt.
# Codon tier: equivalent. Reads FASTA from stdin (downstream of fasta.py).

# The Computer Language Benchmarks Game
# https://salsa.debian.org/benchmarksgame-team/benchmarksgame/
#
# contributed by Matt Vollrath

from itertools import starmap
from sys import stdin, stdout


COMPLEMENTS = bytes.maketrans(
    b'ACGTUMRWSYKVHDBNacgtumrwsykvhdbn',
    b'TGCAAKYWSRMBDHVNTGCAAKYWSRMBDHVN',
)
COMMENT = ord('>')


def reverse_sequence(heading, sequence):
    chunk = bytearray(heading)
    translated = sequence.translate(COMPLEMENTS, b'\n')
    translated.reverse()
    for i in range(0, len(translated), 60):
        chunk += translated[i:i+60] + b'\n'
    return chunk


def generate_sequences(lines):
    heading = None
    sequence = bytearray()
    for line in lines:
        if line[0] == COMMENT:
            if heading:
                yield heading, sequence
                sequence = bytearray()
            heading = line
        else:
            sequence += line
    yield heading, sequence


if __name__ == '__main__':
    sequences = generate_sequences(stdin.buffer)
    for chunk in starmap(reverse_sequence, sequences):
        stdout.buffer.write(chunk)

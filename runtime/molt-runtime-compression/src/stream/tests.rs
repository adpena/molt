use super::*;

// Literal streams produced by CPython 3.12.15, matching the independent
// differential fixture. Expectations do not use a Molt compressor.
const BZ2_MEMBER: &str = "425a683131415926535959aebed50000ce7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffc0021c000126000980009800000000000000000000000000000000000000000000000000000000930004c0004c000000000000000000000000000000000000000000000000000000004980026000260000000000000000000000000000000000000000000000000000000005555268d34686898269a64c460001313004cd1a26689869a0d2698680680002604c000230232600989826264c00230002326000000035212388123e480822424192224642119252224c429272507c04a49512b2584b4f84971f112f3e32604c4f90991332687ca7cc4d49b1372704e49d13b2784f49f13f28046c650ca21f394528c51cfa0a414929452ca614d29c53ca81f49512a4431155421caa9563ea2ae5608820eb25688f7d64315b2b8574af11257c898a256580b09622c6459643ec2ca7da598b39682c5778c206d445dce94486ed6d804196e2de5c08d2e25c885b990c49ae849cba928bb10e949779247a01045e0bc97a2f65f0be97e2fe600fb8c0982306610c29863ef30e620c498a316630c698e31e640c8992326650ca9963f03f132e660cc99a336670fc8fccce99e3f433e680d09a2346690d29a634e7ea6a0d49aa3f63566b0d69ae35e7ee6c0d89b23f83666d0fe4da9b636e6e0dc9ba3766f0de9be37e700fe8fece09c2386710e29c638e720e49ca396730e69ce3fc3fd39e740ff8e89d23a6750ea9d63ae760ec9da3b6770ee9de3be780f09e23c6790f29e63ce7a0f49ea3d67b0f69ee3de471ff8bb9229c28482cd75f6a80";
const XZ_MEMBER: &str = "fd377a585a000004e6d6b446020021010c0000008f98419ce0041801085d00319bc8b0567f59bfde125ea5e4a89ff47cbf6edfd1d37e270e5238ddfd9246b9a4fae6a6dcc27250e0543d0ac5452549c934efb95e3ddd11421a82de9d89379b4038349c1be57d40c9ea859570fef3020c6bec83dee7a5bfd4afab3c6e434572f797470fcd7f391c944b09332c9d293131b1094be00125d7b76866b818ca21ade1b8b6fad33849a23c61aee6a7b88a1df8908f877a103dbc10e4f618190f3840eee76c1abd1f971d7792157c03c13d1551b7cae77f39251b42197a52dec76550123ab6897f7facf4f1a79ea499c4e5dc3f8950f9ba74ea155531df5130596ca139165c8e66de230c2025423af6429fd1924da341591033470fa9555bb6afea427f97bb17448c4800000e309d4f176ed2020001a40299080000d32b43f5b1c467fb020000000004595a";
const ALONE_MEMBER: &str = "5d00000400ffffffffffffffff00319bc8b0567f59bfde125ea5e4a89ff47cbf6edfd1d37e270e5238ddfd9246b9a4fae6a6dcc27250e0543d0ac5452549c934efb95e3ddd11421a82de9d89379b4038349c1be57d40c9ea859570fef3020c6bec83dee7a5bfd4afab3c6e434572f797470fcd7f391c944b09332c9d293131b1094be00125d7b76866b818ca21ade1b8b6fad33849a23c61aee6a7b88a1df8908f877a103dbc10e4f618190f3840eee76c1abd1f971d7792157c03c13d1551b7cae77f39251b42197a52dec76550123ab6897f7facf4f1a79ea499c4e5dc3f8950f9ba74ea155531df5130596ca139165c8e66de230c2025423af6429fd1924da341591033470fa9555bb6afea427f97bb174a77a9f7ffffb8bc6a00";
fn fixture(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn payload() -> Vec<u8> {
    let mut value = b"codec-stream-state\n".to_vec();
    for _ in 0..4 {
        value.extend(0u8..=255);
    }
    value.extend_from_slice(b"\0TAIL\n");
    value
}

fn bounded<D: Decoder>(engine: D, encoded: &[u8], chunk: usize, limit: i64) {
    let mut decoder = StreamDecoder::new(engine);
    let mut input = encoded.to_vec();
    input.extend_from_slice(b"unused suffix");
    let mut cursor = 0;
    let mut output = Vec::new();
    let mut steps = 0;
    while !decoder.eof {
        steps += 1;
        assert!(steps < 10_000, "decoder made no bounded progress");
        let end = if decoder.needs_input {
            (cursor + chunk).min(input.len())
        } else {
            cursor
        };
        let data = &input[cursor..end];
        cursor = end;
        if limit == 0 && !data.is_empty() {
            assert!(decoder.decompress(data, 0).unwrap().is_empty());
            if !decoder.eof {
                let out = decoder.decompress(&[], 31).unwrap();
                assert!(out.len() <= 31);
                output.extend_from_slice(&out);
            }
        } else {
            let max_length = if limit == 0 { 31 } else { limit };
            let out = decoder.decompress(data, max_length).unwrap();
            assert!(max_length < 0 || out.len() <= max_length as usize);
            output.extend_from_slice(&out);
        }
        assert!(
            cursor < input.len() || !decoder.needs_input || decoder.eof,
            "complete stream exhausted without EOF"
        );
    }
    assert_eq!(output, payload());
    let mut tail = decoder.unused_data().to_vec();
    tail.extend_from_slice(&input[cursor..]);
    assert_eq!(tail, b"unused suffix");
    assert!(!decoder.needs_input);
    assert_eq!(decoder.decompress(b"new input", 1), Err(DecodeError::Eof));
}

#[test]
fn bz2_chunked_bounded_literal_stream() {
    let input = fixture(BZ2_MEMBER);
    for chunk in [1, 7, input.len() + 13] {
        for limit in [-1, 0, 1, 31, 8192] {
            bounded(bzip2::Decompress::new(false), &input, chunk, limit);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn lzma_chunked_bounded_literal_streams() {
    for chunk in [1, 7, usize::MAX / 2] {
        for limit in [-1, 0, 1, 31, 8192] {
            bounded(
                xz2::stream::Stream::new_stream_decoder(u64::MAX, 0).unwrap(),
                &fixture(XZ_MEMBER),
                chunk,
                limit,
            );
            bounded(
                xz2::stream::Stream::new_auto_decoder(u64::MAX, 0).unwrap(),
                &fixture(XZ_MEMBER),
                chunk,
                limit,
            );
            bounded(
                xz2::stream::Stream::new_auto_decoder(u64::MAX, 0).unwrap(),
                &fixture(ALONE_MEMBER),
                chunk,
                limit,
            );
            bounded(
                xz2::stream::Stream::new_lzma_decoder(u64::MAX).unwrap(),
                &fixture(ALONE_MEMBER),
                chunk,
                limit,
            );
        }
    }
}

#[test]
fn bz2_corrupt_is_an_error_and_truncated_is_not_eof() {
    let mut decoder = StreamDecoder::new(bzip2::Decompress::new(false));
    assert_eq!(
        decoder.decompress(b"not bz2", -1),
        Err(DecodeError::Codec("Invalid data stream"))
    );
    let mut decoder = StreamDecoder::new(bzip2::Decompress::new(false));
    let encoded = fixture(BZ2_MEMBER);
    decoder
        .decompress(&encoded[..encoded.len() / 2], -1)
        .unwrap();
    assert!(!decoder.eof);
    assert!(decoder.needs_input);
    assert!(decoder.decompress(&[], -1).unwrap().is_empty());
    assert!(!decoder.eof);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn lzma_memory_limit_is_enforced() {
    let mut decoder = StreamDecoder::new(xz2::stream::Stream::new_auto_decoder(1, 0).unwrap());
    assert_eq!(
        decoder.decompress(&fixture(XZ_MEMBER), -1),
        Err(DecodeError::Codec("Memory usage limit exceeded"))
    );
}

#[test]
fn bz2_bounded_output_is_not_eager_and_never_replays_input() {
    let input = fixture(BZ2_MEMBER);
    let mut decoder = StreamDecoder::new(bzip2::Decompress::new(false));
    let mut output = decoder.decompress(&input, 1).unwrap();
    assert_eq!(decoder.engine.total_out(), 1);
    assert!(output.capacity() <= 8);
    while !decoder.eof {
        let next = decoder.decompress(&[], 1).unwrap();
        assert!(next.len() <= 1);
        output.extend_from_slice(&next);
        assert_eq!(decoder.engine.total_out(), output.len() as u64);
        assert!(decoder.engine.total_in() <= input.len() as u64);
    }
    assert_eq!(decoder.engine.total_in(), input.len() as u64);
    assert_eq!(output, payload());
    assert!(decoder.pending.is_empty());
    assert_eq!(decoder.pending.capacity(), 0);
}

#[test]
fn engine_error_after_progress_retires_pending_input() {
    struct FailAfterProgress {
        seen: Vec<u8>,
        fail: bool,
    }
    impl Decoder for FailAfterProgress {
        const DRAIN_AFTER_FULL_OUTPUT: bool = false;
        fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecodeStep, DecodeError> {
            if !self.seen.is_empty() && self.fail {
                self.fail = false;
                return Err(DecodeError::Memory);
            }
            let size = usize::from(!input.is_empty() && !output.is_empty());
            if size != 0 {
                self.seen.push(input[0]);
                output[0] = input[0];
            }
            Ok(DecodeStep {
                consumed: size,
                written: size,
                end: false,
            })
        }
    }
    let mut decoder = StreamDecoder::new(FailAfterProgress {
        seen: Vec::new(),
        fail: true,
    });
    assert!(decoder.decompress(b"abc", 0).unwrap().is_empty());
    assert_eq!(decoder.decompress(&[], -1), Err(DecodeError::Memory));
    assert!(decoder.pending.is_empty());
    assert_eq!(decoder.engine.seen, b"a");
    assert_eq!(decoder.decompress(b"d", -1).unwrap(), b"d");
    assert_eq!(decoder.engine.seen, b"ad");
}

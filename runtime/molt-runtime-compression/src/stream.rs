//! Shared CPython incremental-decoder policy, separate from codec engines.
//!
//! `_bz2module.c` and `_lzmamodule.c` retain one engine and only unconsumed
//! compressed input. They never replay a prefix or retain decoded history.
//! https://github.com/python/cpython/blob/v3.12.13/Modules/_bz2module.c
//! https://github.com/python/cpython/blob/v3.12.13/Modules/_lzmamodule.c

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeError {
    Memory,
    Codec(&'static str),
    Value(&'static str),
    Runtime(&'static str),
    Eof,
}

pub(crate) struct DecodeStep {
    pub consumed: usize,
    pub written: usize,
    pub end: bool,
}

pub(crate) trait Decoder {
    // liblzma may have internal output after consuming the last input byte;
    // CPython's BZ2 state instead derives needs_input from avail_in alone.
    const DRAIN_AFTER_FULL_OUTPUT: bool;
    fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecodeStep, DecodeError>;
}

pub(crate) struct StreamDecoder<D> {
    engine: D,
    pending: Vec<u8>,
    pending_start: usize,
    pub eof: bool,
    pub needs_input: bool,
}

impl<D: Decoder> StreamDecoder<D> {
    pub fn new(engine: D) -> Self {
        Self {
            engine,
            pending: Vec::new(),
            pending_start: 0,
            eof: false,
            needs_input: true,
        }
    }

    pub fn unused_data(&self) -> &[u8] {
        if self.eof {
            &self.pending[self.pending_start..]
        } else {
            &[]
        }
    }

    pub fn decompress(&mut self, data: &[u8], max_length: i64) -> Result<Vec<u8>, DecodeError> {
        if self.eof {
            return Err(DecodeError::Eof);
        }
        let limit = if max_length < 0 {
            usize::MAX
        } else {
            usize::try_from(max_length).map_err(|_| DecodeError::Memory)?
        };
        // Borrow fresh input. Only a tail that survives this call needs an
        // owned copy. A cursor avoids shifting the whole pending buffer for
        // every bounded output read; compaction is amortized over consumption.
        let owned = self.pending_start < self.pending.len();
        if owned && !data.is_empty() {
            if self.pending_start >= self.pending.len() / 2 {
                self.pending.copy_within(self.pending_start.., 0);
                self.pending
                    .truncate(self.pending.len() - self.pending_start);
                self.pending_start = 0;
            }
            self.pending
                .try_reserve(data.len())
                .map_err(|_| DecodeError::Memory)?;
            self.pending.extend_from_slice(data);
        }
        let input = if owned {
            &self.pending[self.pending_start..]
        } else {
            data
        };
        let mut consumed = 0;
        let mut output = Vec::new();
        let mut processed = false;
        let result = (|| {
            loop {
                // Grow geometrically without allocating max_length up front. The
                // engine sees exactly this window, even if Vec reserves more.
                let window = limit
                    .saturating_sub(output.len())
                    .min(output.len().clamp(8192, 1024 * 1024));
                let before = output.len();
                output
                    .try_reserve(window)
                    .map_err(|_| DecodeError::Memory)?;
                output.resize(before + window, 0);
                // A zero-length request still lets the engine consume headers and
                // report errors, as CPython does; it cannot materialize output.
                processed = true;
                let step = self
                    .engine
                    .process(&input[consumed..], &mut output[before..])?;
                consumed += step.consumed;
                output.truncate(before + step.written);
                self.eof = step.end;
                let full = step.written == window;
                if step.end
                    || output.len() == limit
                    || (consumed == input.len() && (!full || !D::DRAIN_AFTER_FULL_OUTPUT))
                    || (step.consumed == 0 && step.written == 0)
                {
                    break Ok(full);
                }
            }
        })();
        let last_output_full = match result {
            Ok(full) => full,
            Err(error) => {
                // CPython clears next_in after a decode failure. An engine may
                // already have advanced, so no later call may replay this tail.
                if processed {
                    self.pending = Vec::new();
                    self.pending_start = 0;
                }
                return Err(error);
            }
        };
        let tail = &input[consumed..];
        self.needs_input =
            !self.eof && tail.is_empty() && !(D::DRAIN_AFTER_FULL_OUTPUT && last_output_full);
        if self.eof || !owned {
            // Retain only the unused suffix; release the consumed allocation
            // at EOF. Allocate before replacing any existing pending owner.
            let mut retained = Vec::new();
            retained
                .try_reserve_exact(tail.len())
                .map_err(|_| DecodeError::Memory)?;
            retained.extend_from_slice(tail);
            self.pending = retained;
            self.pending_start = 0;
        } else {
            self.pending_start += consumed;
            if self.pending_start == self.pending.len() {
                self.pending = Vec::new();
                self.pending_start = 0;
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests;

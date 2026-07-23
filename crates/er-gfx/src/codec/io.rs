struct GfxReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> GfxReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        GfxReader { data, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<(), GfxError> {
        if self.pos + n > self.data.len() {
            Err(GfxError::UnexpectedEof {
                pos: self.pos,
                need: n,
                have: self.data.len().saturating_sub(self.pos),
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8, GfxError> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16, GfxError> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, GfxError> {
        self.need(4)?;
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, GfxError> {
        self.need(n)?;
        let s = self.data[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(s)
    }

    /// Read a NUL-terminated string, consuming the terminator. The returned
    /// `String` excludes the NUL (re-added by the writer). Errors loudly if the
    /// data ends before a NUL or the bytes are not valid UTF-8 -- `code` names
    /// the owning tag for diagnostics.
    fn read_cstring(&mut self, code: u16) -> Result<String, GfxError> {
        let start = self.pos;
        loop {
            if self.pos >= self.data.len() {
                return Err(GfxError::UnterminatedString { code });
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            if byte == 0 {
                let bytes = self.data[start..self.pos - 1].to_vec();
                return String::from_utf8(bytes).map_err(|_| GfxError::InvalidUtf8 { code });
            }
        }
    }

    /// Read the bit-packed movie `RECT` as raw bytes.
    ///
    /// Layout: top 5 bits of the first byte are `Nbits`; the field then holds
    /// `4 * Nbits` more bits (signed xmin/xmax/ymin/ymax) for `5 + 4*Nbits`
    /// total bits, byte-aligned. We only compute the byte length and slice the
    /// bytes verbatim (no bit decode), preserving them exactly.
    fn read_rect_raw(&mut self) -> Result<Vec<u8>, GfxError> {
        self.need(1)?;
        let nbits = (self.data[self.pos] >> 3) as usize;
        let total_bits = 5 + 4 * nbits;
        let byte_len = total_bits.div_ceil(8);
        self.read_bytes(byte_len)
    }
}

/// Append-only byte sink with the small write helpers the writer needs.
struct GfxWriter {
    buf: Vec<u8>,
}

impl GfxWriter {
    fn new() -> Self {
        GfxWriter { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Write a string followed by its NUL terminator.
    fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// Emit a `RecordHeader` for `code`/`body_len`, honoring `force_long`.
    fn write_record_header(
        &mut self,
        code: u16,
        body_len: usize,
        force_long: bool,
    ) -> Result<(), GfxError> {
        if code > MAX_TAG_CODE {
            return Err(GfxError::CodeTooLarge(code));
        }
        if force_long || body_len >= LONG_LEN_SENTINEL as usize {
            let word = (code << 6) | LONG_LEN_SENTINEL;
            self.write_u16(word);
            self.write_u32(body_len as u32);
        } else {
            let word = (code << 6) | (body_len as u16);
            self.write_u16(word);
        }
        Ok(())
    }
}

// ===========================================================================
// Tier-2 bit-packed primitive layer (MSB-first bit order -- the SWF convention)
// ===========================================================================
//
// SWF/GFX bit-packed structures (RECT, MATRIX, CXFORM[WITHALPHA]) read bits
// most-significant-first and byte-align at the end of each structure. The fields
// `Nbits` (RECT), `NScaleBits`/`NRotateBits`/`NTranslateBits` (MATRIX), and
// `Nbits` (CXFORM) are NOT guaranteed minimal: the Scaleform exporter is
// confirmed non-minimal (2,413 MATRIX instances in the 114-file corpus use more
// translate bits than the minimal width; 21 scale, 14 rotate). Byte-identity
// therefore REQUIRES storing each source nbits verbatim and re-encoding with it,
// never recomputing a minimal width. Byte-alignment padding is always zero
// across the corpus (0 non-zero pads over 171,728 MATRIX, 20,157 CXFORM, 124
// RECT); the reader rejects a non-zero pad ([`GfxError::NonZeroBitPadding`])
// rather than silently dropping bits, and the writer zero-fills.

/// MSB-first bit cursor over a byte slice (typically a single tag body).
struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position from the start of `data`.
    bitpos: usize,
}

impl<'a> BitReader<'a> {
    /// Construct a reader positioned at byte `byte_off` (bit-aligned). Bit-packed
    /// SWF/GFX structures always begin on a byte boundary.
    fn new_at_byte(data: &'a [u8], byte_off: usize) -> Self {
        BitReader {
            data,
            bitpos: byte_off * 8,
        }
    }

    /// Read `n` (<= 32) unsigned bits, MSB-first.
    fn read_ubits(&mut self, n: u32, context: &'static str) -> Result<u32, GfxError> {
        let mut acc: u64 = 0;
        for _ in 0..n {
            let byte_idx = self.bitpos >> 3;
            if byte_idx >= self.data.len() {
                return Err(GfxError::BitstreamEof { context });
            }
            let bit = (self.data[byte_idx] >> (7 - (self.bitpos & 7))) & 1;
            acc = (acc << 1) | bit as u64;
            self.bitpos += 1;
        }
        Ok(acc as u32)
    }

    /// Read `n` (<= 32) bits and sign-extend (two's complement).
    fn read_sbits(&mut self, n: u32, context: &'static str) -> Result<i32, GfxError> {
        if n == 0 {
            return Ok(0);
        }
        let u = self.read_ubits(n, context)?;
        let v = if n < 32 && (u & (1u32 << (n - 1))) != 0 {
            // Sign bit set: extend the high bits.
            (u | !((1u32 << n) - 1)) as i32
        } else {
            u as i32
        };
        Ok(v)
    }

    /// Read an `FB` fixed-point value as its raw signed integer (16.16 fixed is a
    /// signed integer at the bit level; callers interpret the scaling). Identical
    /// bit handling to [`read_sbits`](Self::read_sbits).
    fn read_fbits(&mut self, n: u32, context: &'static str) -> Result<i32, GfxError> {
        self.read_sbits(n, context)
    }

    /// Consume padding bits up to the next byte boundary. The padding must be
    /// zero (corpus-proven) or this errors loudly.
    fn byte_align(&mut self, context: &'static str) -> Result<(), GfxError> {
        let rem = (8 - (self.bitpos & 7)) & 7;
        let pad = self.read_ubits(rem as u32, context)?;
        if pad != 0 {
            return Err(GfxError::NonZeroBitPadding { context });
        }
        Ok(())
    }

    /// Current byte offset (must be byte-aligned, e.g. just after `byte_align`).
    fn byte_pos(&self) -> usize {
        debug_assert_eq!(self.bitpos & 7, 0, "BitReader not byte aligned");
        self.bitpos >> 3
    }

    /// Read one whole byte at the current (byte-aligned) position. Used by the
    /// shape codec, whose FILLSTYLEARRAY/LINESTYLEARRAY/GRADRECORD fields are
    /// byte-structured even though they are embedded in the larger bitstream.
    fn read_u8_aligned(&mut self, context: &'static str) -> Result<u8, GfxError> {
        debug_assert_eq!(self.bitpos & 7, 0, "read_u8_aligned not byte aligned");
        let idx = self.bitpos >> 3;
        if idx >= self.data.len() {
            return Err(GfxError::BitstreamEof { context });
        }
        let v = self.data[idx];
        self.bitpos += 8;
        Ok(v)
    }

    /// Read a little-endian `u16` at the current (byte-aligned) position.
    fn read_u16_aligned(&mut self, context: &'static str) -> Result<u16, GfxError> {
        let lo = self.read_u8_aligned(context)?;
        let hi = self.read_u8_aligned(context)?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    /// Read `n` whole bytes at the current (byte-aligned) position.
    fn read_bytes_aligned(&mut self, n: usize, context: &'static str) -> Result<Vec<u8>, GfxError> {
        debug_assert_eq!(self.bitpos & 7, 0, "read_bytes_aligned not byte aligned");
        let idx = self.bitpos >> 3;
        if idx + n > self.data.len() {
            return Err(GfxError::BitstreamEof { context });
        }
        let s = self.data[idx..idx + n].to_vec();
        self.bitpos += n * 8;
        Ok(s)
    }
}

/// MSB-first bit sink. Bits accumulate into a current byte (MSB-first) and flush
/// on each completed byte; [`byte_align`](Self::byte_align) zero-fills.
struct BitWriter {
    buf: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            buf: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    fn write_bit(&mut self, bit: u32) {
        self.cur = (self.cur << 1) | (bit as u8 & 1);
        self.nbits += 1;
        if self.nbits == 8 {
            self.buf.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn write_ubits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bit((value >> i) & 1);
        }
    }

    fn write_sbits(&mut self, value: i32, n: u32) {
        if n == 0 {
            return;
        }
        let mask = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
        self.write_ubits((value as u32) & mask, n);
    }

    fn write_fbits(&mut self, value: i32, n: u32) {
        self.write_sbits(value, n);
    }

    /// Zero-fill to the next byte boundary.
    fn byte_align(&mut self) {
        while self.nbits != 0 {
            self.write_bit(0);
        }
    }

    /// Write one whole byte at the current (byte-aligned) position. Counterpart
    /// to [`BitReader::read_u8_aligned`]; used by the shape codec.
    fn write_u8_aligned(&mut self, v: u8) {
        debug_assert_eq!(self.nbits, 0, "write_u8_aligned not byte aligned");
        self.buf.push(v);
    }

    /// Write a little-endian `u16` at the current (byte-aligned) position.
    fn write_u16_aligned(&mut self, v: u16) {
        self.write_u8_aligned((v & 0xff) as u8);
        self.write_u8_aligned((v >> 8) as u8);
    }

    /// Write whole bytes at the current (byte-aligned) position.
    fn write_bytes_aligned(&mut self, b: &[u8]) {
        debug_assert_eq!(self.nbits, 0, "write_bytes_aligned not byte aligned");
        self.buf.extend_from_slice(b);
    }

    /// Finish, asserting byte alignment, and return the bytes.
    fn into_bytes(self) -> Vec<u8> {
        debug_assert_eq!(self.nbits, 0, "BitWriter not byte aligned");
        self.buf
    }
}

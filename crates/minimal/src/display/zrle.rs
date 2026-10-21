//! ZRLE (RFB encoding 16): the encoding `min session display` asks the box's
//! VNC server for first, decoded into the client's framebuffer.
//!
//! A ZRLE rect is a length-prefixed slice of one zlib stream that lives as
//! long as the connection, so the decoder keeps its inflater between rects.
//! The inflated bytes describe the rect in 64×64 tiles (smaller at the right
//! and bottom edges), each a subencoding byte and its data. Pixels are
//! CPIXELs: the pixel format this client sets (32 bpp, depth 24, little
//! endian, red at bit 16) makes each one three bytes, blue first.
//!
//! Every byte here comes from inside the box, so nothing is trusted: a short
//! tile, a run past its tile, an unknown subencoding or an inflate larger
//! than the rect could ever need is an error, never a panic.

use flate2::{Decompress, FlushDecompress, Status};

use super::rfb::{Framebuffer, Rect, RfbError};

/// Tiles are this many pixels on a side, except at the rect's right and
/// bottom edges.
const TILE: u16 = 64;

/// The most inflated bytes one tile can take: its subencoding byte, a
/// 127-colour palette, and a plain-RLE tile of single-pixel runs (a CPIXEL
/// and one length byte per pixel), the largest of the subencodings.
const MAX_TILE_BYTES: usize = 1 + 127 * 3 + (TILE as usize * TILE as usize) * 4;

/// The connection's ZRLE state: one zlib stream for every rect.
pub(crate) struct ZrleDecoder {
    inflate: Decompress,
    /// Reused between rects, so a steady stream of updates allocates once.
    scratch: Vec<u8>,
}

impl ZrleDecoder {
    pub(crate) fn new() -> Self {
        Self {
            inflate: Decompress::new(true),
            scratch: Vec::new(),
        }
    }

    /// Decodes one ZRLE rect's `compressed` data into `fb` at `rect`, which
    /// the caller has checked lies inside `fb`.
    pub(crate) fn decode_rect(
        &mut self,
        compressed: &[u8],
        rect: Rect,
        fb: &mut Framebuffer,
    ) -> Result<(), RfbError> {
        let limit = tile_count(rect).saturating_mul(MAX_TILE_BYTES);
        self.inflate_rect(compressed, limit)?;
        let mut tiles = Cursor::new(&self.scratch);
        for ty in (0..rect.height).step_by(usize::from(TILE)) {
            for tx in (0..rect.width).step_by(usize::from(TILE)) {
                let tile = Rect {
                    x: rect.x + tx,
                    y: rect.y + ty,
                    width: TILE.min(rect.width - tx),
                    height: TILE.min(rect.height - ty),
                };
                decode_tile(&mut tiles, tile, fb)?;
            }
        }
        Ok(())
    }

    /// Inflates all of `compressed` into `self.scratch`, refusing more than
    /// `limit` bytes of output.
    ///
    /// The server flushes the stream at the end of every rect, so once the
    /// input is consumed every byte of this rect's output is out too.
    fn inflate_rect(&mut self, compressed: &[u8], limit: usize) -> Result<(), RfbError> {
        self.scratch.clear();
        let mut consumed = 0;
        loop {
            if self.scratch.len() >= limit {
                return Err(RfbError::protocol(
                    "a ZRLE rect inflated larger than its tiles can be",
                ));
            }
            let room = (limit - self.scratch.len()).min(64 * 1024);
            self.scratch.reserve(room);
            let before_in = self.inflate.total_in();
            let before_out = self.scratch.len();
            let status = self
                .inflate
                .decompress_vec(
                    compressed.get(consumed..).unwrap_or_default(),
                    &mut self.scratch,
                    FlushDecompress::Sync,
                )
                .map_err(|e| RfbError::protocol(format!("bad ZRLE zlib data: {e}")))?;
            consumed += usize::try_from(self.inflate.total_in() - before_in)
                .expect("one rect's input fits in memory");
            let produced = self.scratch.len() - before_out;
            if status == Status::StreamEnd {
                return Err(RfbError::protocol("the ZRLE zlib stream ended"));
            }
            // Done when the input is used up and the last call left output
            // room unused: nothing more is buffered inside the inflater.
            if consumed >= compressed.len() && produced < room {
                return Ok(());
            }
            if produced == 0 && consumed < compressed.len() && status == Status::BufError {
                return Err(RfbError::protocol("the ZRLE zlib stream stalled"));
            }
        }
    }
}

/// How many tiles `rect` is cut into.
fn tile_count(rect: Rect) -> usize {
    let across = usize::from(rect.width).div_ceil(usize::from(TILE));
    let down = usize::from(rect.height).div_ceil(usize::from(TILE));
    across * down
}

/// A read position in the inflated bytes, failing rather than running off
/// the end.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn u8(&mut self) -> Result<u8, RfbError> {
        let byte = *self
            .bytes
            .get(self.at)
            .ok_or_else(|| RfbError::protocol("a ZRLE tile ended early"))?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], RfbError> {
        let end = self.at.checked_add(n);
        let slice = end
            .and_then(|end| self.bytes.get(self.at..end))
            .ok_or_else(|| RfbError::protocol("a ZRLE tile ended early"))?;
        self.at += n;
        Ok(slice)
    }

    /// One CPIXEL: blue, green, red.
    fn cpixel(&mut self) -> Result<u32, RfbError> {
        let [b, g, r] = self
            .take(3)?
            .try_into()
            .expect("take(3) returns three bytes");
        Ok(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
    }

    /// A run length: bytes summed while they are 255, plus the one that is
    /// not, plus one.
    fn run_length(&mut self) -> Result<usize, RfbError> {
        let mut length = 1usize;
        loop {
            let byte = self.u8()?;
            length = length.saturating_add(usize::from(byte));
            if byte != 255 {
                return Ok(length);
            }
        }
    }

    fn palette(&mut self, size: u8) -> Result<Vec<u32>, RfbError> {
        (0..size).map(|_| self.cpixel()).collect()
    }
}

/// Decodes one tile's subencoding and data into `fb`.
fn decode_tile(tiles: &mut Cursor<'_>, tile: Rect, fb: &mut Framebuffer) -> Result<(), RfbError> {
    let pixels = usize::from(tile.width) * usize::from(tile.height);
    let mut out = TileWriter::new(tile, fb);
    match tiles.u8()? {
        // Raw: every pixel in turn.
        0 => {
            for _ in 0..pixels {
                out.push(tiles.cpixel()?, 1)?;
            }
        }
        // Solid: one colour for the whole tile.
        1 => out.push(tiles.cpixel()?, pixels)?,
        // Packed palette: 2..=16 colours, each pixel an index of 1, 2 or 4
        // bits, most significant first, every row padded to a whole byte.
        size @ 2..=16 => {
            let palette = tiles.palette(size)?;
            let bits: usize = match size {
                2 => 1,
                3..=4 => 2,
                _ => 4,
            };
            let row_bytes = (usize::from(tile.width) * bits).div_ceil(8);
            for _ in 0..tile.height {
                let row = tiles.take(row_bytes)?;
                for col in 0..usize::from(tile.width) {
                    let bit = col * bits;
                    let byte = row[bit / 8];
                    let shift = 8 - bits - bit % 8;
                    let index = usize::from(byte >> shift) & ((1 << bits) - 1);
                    let colour = *palette
                        .get(index)
                        .ok_or_else(|| RfbError::protocol("a ZRLE index past its palette"))?;
                    out.push(colour, 1)?;
                }
            }
        }
        // Plain RLE: runs of one colour each.
        128 => {
            while out.remaining() > 0 {
                let colour = tiles.cpixel()?;
                let length = tiles.run_length()?;
                out.push(colour, length)?;
            }
        }
        // Palette RLE: 2..=127 colours; an index with its top bit set is
        // followed by a run length, otherwise it covers one pixel.
        size @ 130..=255 => {
            let palette = tiles.palette(size - 128)?;
            while out.remaining() > 0 {
                let entry = tiles.u8()?;
                let colour = *palette
                    .get(usize::from(entry & 0x7f))
                    .ok_or_else(|| RfbError::protocol("a ZRLE index past its palette"))?;
                let length = if entry & 0x80 == 0 {
                    1
                } else {
                    tiles.run_length()?
                };
                out.push(colour, length)?;
            }
        }
        other => {
            return Err(RfbError::protocol(format!(
                "unknown ZRLE subencoding {other}"
            )));
        }
    }
    Ok(())
}

/// Fills one tile of the framebuffer in row order.
struct TileWriter<'a> {
    tile: Rect,
    fb: &'a mut Framebuffer,
    /// Pixels written so far, in row order within the tile.
    done: usize,
}

impl<'a> TileWriter<'a> {
    fn new(tile: Rect, fb: &'a mut Framebuffer) -> Self {
        Self { tile, fb, done: 0 }
    }

    fn total(&self) -> usize {
        usize::from(self.tile.width) * usize::from(self.tile.height)
    }

    fn remaining(&self) -> usize {
        self.total() - self.done
    }

    /// Writes `colour` to the next `count` pixels.
    fn push(&mut self, colour: u32, count: usize) -> Result<(), RfbError> {
        if count > self.remaining() {
            return Err(RfbError::protocol("a ZRLE run past the end of its tile"));
        }
        let width = usize::from(self.tile.width);
        for n in self.done..self.done + count {
            let x = self.tile.x + u16::try_from(n % width).expect("a column within a tile");
            let y = self.tile.y + u16::try_from(n / width).expect("a row within a tile");
            self.fb.set(x, y, colour);
        }
        self.done += count;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use flate2::{Compress, Compression, FlushCompress};

    use super::*;

    /// A server-side zlib stream: each rect is compressed and sync-flushed
    /// onto the one stream, as neatvnc does.
    struct Server(Compress);

    impl Server {
        fn new() -> Self {
            Self(Compress::new(Compression::default(), true))
        }

        fn rect(&mut self, tiles: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(tiles.len() + 64);
            loop {
                let consumed = usize::try_from(self.0.total_in()).unwrap();
                let start_in = consumed;
                out.reserve(1024);
                self.0
                    .compress_vec(&tiles[..], &mut out, FlushCompress::Sync)
                    .unwrap();
                let used = usize::try_from(self.0.total_in()).unwrap() - start_in;
                if used == tiles.len() && out.len() < out.capacity() {
                    return out;
                }
                assert_eq!(used, tiles.len(), "small test inputs compress in one call");
            }
        }
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A CPIXEL for `0x00RRGGBB`.
    fn cpixel(rgb: u32) -> [u8; 3] {
        let [b, g, r, _] = rgb.to_le_bytes();
        [b, g, r]
    }

    fn decode(tiles: &[u8], r: Rect, fb: &mut Framebuffer) -> Result<(), RfbError> {
        let compressed = Server::new().rect(tiles);
        ZrleDecoder::new().decode_rect(&compressed, r, fb)
    }

    #[test]
    fn a_solid_tile_fills_its_rect() {
        let mut fb = Framebuffer::new(4, 4);
        let mut tiles = vec![1];
        tiles.extend(cpixel(0x00ab_cdef));
        decode(&tiles, rect(1, 1, 2, 2), &mut fb).unwrap();
        assert_eq!(fb.get(1, 1), 0x00ab_cdef);
        assert_eq!(fb.get(2, 2), 0x00ab_cdef);
        assert_eq!(fb.get(0, 0), 0, "outside the rect is untouched");
        assert_eq!(fb.get(3, 3), 0, "outside the rect is untouched");
    }

    #[test]
    fn a_raw_tile_is_pixels_in_row_order() {
        let mut fb = Framebuffer::new(2, 2);
        let mut tiles = vec![0];
        for rgb in [0x11, 0x22, 0x33, 0x44] {
            tiles.extend(cpixel(rgb));
        }
        decode(&tiles, rect(0, 0, 2, 2), &mut fb).unwrap();
        assert_eq!(
            [fb.get(0, 0), fb.get(1, 0), fb.get(0, 1), fb.get(1, 1)],
            [0x11, 0x22, 0x33, 0x44]
        );
    }

    #[test]
    fn a_two_colour_palette_packs_one_bit_per_pixel_with_padded_rows() {
        // 3 wide: each row is 3 bits, padded to one byte.
        let mut fb = Framebuffer::new(3, 2);
        let mut tiles = vec![2];
        tiles.extend(cpixel(0xaa));
        tiles.extend(cpixel(0xbb));
        tiles.extend([0b1010_0000, 0b0100_0000]);
        decode(&tiles, rect(0, 0, 3, 2), &mut fb).unwrap();
        assert_eq!([fb.get(0, 0), fb.get(1, 0), fb.get(2, 0)], [0xbb, 0xaa, 0xbb]);
        assert_eq!([fb.get(0, 1), fb.get(1, 1), fb.get(2, 1)], [0xaa, 0xbb, 0xaa]);
    }

    #[test]
    fn a_five_colour_palette_packs_four_bits_per_pixel() {
        let mut fb = Framebuffer::new(2, 1);
        let mut tiles = vec![5];
        for rgb in [1, 2, 3, 4, 5] {
            tiles.extend(cpixel(rgb));
        }
        tiles.push(0x41);
        decode(&tiles, rect(0, 0, 2, 1), &mut fb).unwrap();
        assert_eq!([fb.get(0, 0), fb.get(1, 0)], [5, 2]);
    }

    #[test]
    fn plain_rle_runs_add_up_their_255s() {
        // One run of 300 pixels: 255 + 44, plus one.
        let mut fb = Framebuffer::new(20, 15);
        let mut tiles = vec![128];
        tiles.extend(cpixel(0x77));
        tiles.extend([255, 44]);
        decode(&tiles, rect(0, 0, 20, 15), &mut fb).unwrap();
        assert_eq!(fb.get(19, 14), 0x77);
    }

    #[test]
    fn palette_rle_mixes_single_pixels_and_runs() {
        let mut fb = Framebuffer::new(4, 1);
        let mut tiles = vec![130];
        tiles.extend(cpixel(0x10));
        tiles.extend(cpixel(0x20));
        // Index 1 once, then index 0 for a run of three.
        tiles.extend([0x01, 0x80, 2]);
        decode(&tiles, rect(0, 0, 4, 1), &mut fb).unwrap();
        assert_eq!(
            [fb.get(0, 0), fb.get(1, 0), fb.get(2, 0), fb.get(3, 0)],
            [0x20, 0x10, 0x10, 0x10]
        );
    }

    #[test]
    fn a_rect_wider_than_a_tile_is_cut_at_64_pixels() {
        // 70×1: a 64-wide tile, then a 6-wide one.
        let mut fb = Framebuffer::new(70, 1);
        let mut tiles = vec![1];
        tiles.extend(cpixel(0x01));
        tiles.push(1);
        tiles.extend(cpixel(0x02));
        decode(&tiles, rect(0, 0, 70, 1), &mut fb).unwrap();
        assert_eq!(fb.get(63, 0), 0x01);
        assert_eq!(fb.get(64, 0), 0x02);
        assert_eq!(fb.get(69, 0), 0x02);
    }

    #[test]
    fn one_zlib_stream_carries_every_rect() {
        let mut server = Server::new();
        let mut decoder = ZrleDecoder::new();
        let mut fb = Framebuffer::new(2, 1);
        for (x, rgb) in [(0, 0x0a), (1, 0x0b)] {
            let mut tiles = vec![1];
            tiles.extend(cpixel(rgb));
            let compressed = server.rect(&tiles);
            decoder
                .decode_rect(&compressed, rect(x, 0, 1, 1), &mut fb)
                .unwrap();
        }
        assert_eq!([fb.get(0, 0), fb.get(1, 0)], [0x0a, 0x0b]);
    }

    #[test]
    fn a_run_past_its_tile_is_an_error() {
        let mut fb = Framebuffer::new(2, 2);
        let mut tiles = vec![128];
        tiles.extend(cpixel(0x01));
        tiles.push(4); // five pixels in a four-pixel tile
        assert!(decode(&tiles, rect(0, 0, 2, 2), &mut fb).is_err());
    }

    #[test]
    fn a_short_tile_is_an_error() {
        let mut fb = Framebuffer::new(2, 2);
        let mut tiles = vec![0];
        tiles.extend(cpixel(0x01)); // one of four pixels
        assert!(decode(&tiles, rect(0, 0, 2, 2), &mut fb).is_err());
    }

    #[test]
    fn subencodings_17_to_127_and_129_are_errors() {
        for subencoding in [17, 127, 129] {
            let mut fb = Framebuffer::new(1, 1);
            let err = decode(&[subencoding, 0, 0, 0], rect(0, 0, 1, 1), &mut fb).unwrap_err();
            assert!(
                err.to_string().contains("subencoding"),
                "{subencoding}: {err}"
            );
        }
    }

    #[test]
    fn an_inflate_larger_than_the_rect_needs_is_an_error() {
        // A 1×1 rect can need at most one tile's worth of bytes; send ten.
        let mut fb = Framebuffer::new(1, 1);
        let tiles = vec![0; MAX_TILE_BYTES * 10];
        let err = decode(&tiles, rect(0, 0, 1, 1), &mut fb).unwrap_err();
        assert!(err.to_string().contains("inflated larger"), "{err}");
    }

    #[test]
    fn garbage_instead_of_zlib_is_an_error() {
        let mut fb = Framebuffer::new(1, 1);
        let err = ZrleDecoder::new()
            .decode_rect(b"not zlib at all", rect(0, 0, 1, 1), &mut fb)
            .unwrap_err();
        assert!(err.to_string().contains("zlib"), "{err}");
    }
}

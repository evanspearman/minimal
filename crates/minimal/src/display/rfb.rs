//! The RFB (VNC) client half of `min session display`: the handshake, the
//! framebuffer the server's updates are decoded into, and the messages the
//! window sends back.
//!
//! The server is the box's wayvnc, reached over an SSH channel, so the
//! client speaks only what wayvnc's library (neatvnc) needs: protocol 3.8,
//! security type None, and frame updates in ZRLE or Raw. neatvnc encodes a
//! frame in the first of Raw, Tight or ZRLE in the client's list, so ZRLE
//! comes first. Size changes arrive as DesktopSize or ExtendedDesktopSize
//! pseudo-rects and are applied in wire order: wayvnc sends the new size
//! and the pixels at that size in one update.
//!
//! Every byte read here comes from inside the box. Sizes are bounded, rects
//! are checked against the framebuffer, and anything unexpected is an
//! [`RfbError`], never a panic.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use super::zrle::ZrleDecoder;

/// The largest framebuffer side the client accepts, in pixels.
pub(crate) const MAX_DIMENSION: u16 = 8192;
/// The longest desktop name the client reads.
const MAX_NAME: u32 = 4096;
/// The largest ZRLE rect the client reads, compressed.
const MAX_ZRLE_BYTES: u32 = 64 * 1024 * 1024;
/// The longest server cut text the client skips over.
const MAX_CUT_TEXT: u32 = 1024 * 1024;

const ENCODING_RAW: i32 = 0;
const ENCODING_ZRLE: i32 = 16;
const ENCODING_DESKTOP_SIZE: i32 = -223;
const ENCODING_EXTENDED_DESKTOP_SIZE: i32 = -308;

/// What the client asks for, in order of preference.
const ENCODINGS: [i32; 4] = [
    ENCODING_ZRLE,
    ENCODING_RAW,
    ENCODING_EXTENDED_DESKTOP_SIZE,
    ENCODING_DESKTOP_SIZE,
];

/// Why the RFB conversation ended.
#[derive(Debug)]
pub(crate) enum RfbError {
    /// The stream failed or ended.
    Io(std::io::Error),
    /// The server sent something this client does not accept.
    Protocol(String),
}

impl RfbError {
    pub(crate) fn protocol(what: impl Into<String>) -> Self {
        Self::Protocol(what.into())
    }
}

impl std::fmt::Display for RfbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Protocol(what) => write!(f, "the VNC server sent {what}"),
        }
    }
}

impl std::error::Error for RfbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Protocol(_) => None,
        }
    }
}

impl From<std::io::Error> for RfbError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A rectangle of the framebuffer, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

/// The remote desktop's pixels, `0x00RRGGBB` in row order: the layout
/// softbuffer presents, so the window copies rows without converting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u32>,
}

impl Framebuffer {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
        }
    }

    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }

    /// Sets one pixel. Callers stay inside the framebuffer: every rect is
    /// checked by [`Framebuffer::check`] before it is decoded.
    pub(crate) fn set(&mut self, x: u16, y: u16, rgb: u32) {
        let i = self.index(x, y);
        if let Some(pixel) = self.pixels.get_mut(i) {
            *pixel = rgb;
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, x: u16, y: u16) -> u32 {
        self.pixels[self.index(x, y)]
    }

    /// Changes the size, keeping what overlaps the old one.
    fn resize(&mut self, width: u16, height: u16) {
        let mut next = Self::new(width, height);
        for y in 0..self.height.min(height) {
            for x in 0..self.width.min(width) {
                next.set(x, y, self.pixels[self.index(x, y)]);
            }
        }
        *self = next;
    }

    /// Fails unless `rect` lies inside the framebuffer.
    fn check(&self, rect: Rect) -> Result<(), RfbError> {
        let inside = u32::from(rect.x) + u32::from(rect.width) <= u32::from(self.width)
            && u32::from(rect.y) + u32::from(rect.height) <= u32::from(self.height);
        if inside {
            Ok(())
        } else {
            Err(RfbError::protocol(format!(
                "a {}x{} rect at {},{} outside its {}x{} desktop",
                rect.width, rect.height, rect.x, rect.y, self.width, self.height
            )))
        }
    }
}

/// What the server said about its desktop once the handshake was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerInit {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) name: String,
}

/// Runs the handshake on `stream` and asks for the first full frame.
///
/// Protocol 3.8, security type None, a shared session; then the pixel
/// format and encodings this client decodes, and a non-incremental update
/// request for the whole desktop.
pub(crate) async fn handshake<S>(stream: &mut S) -> Result<ServerInit, RfbError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut version = [0u8; 12];
    stream.read_exact(&mut version).await?;
    let minor = std::str::from_utf8(&version)
        .ok()
        .and_then(|v| v.strip_prefix("RFB 003."))
        .and_then(|v| v.strip_suffix('\n'))
        .and_then(|v| v.parse::<u16>().ok());
    if !minor.is_some_and(|minor| minor >= 8) {
        return Err(RfbError::protocol(format!(
            "protocol version {:?}; this client needs RFB 3.8",
            String::from_utf8_lossy(&version).trim_end()
        )));
    }
    stream.write_all(b"RFB 003.008\n").await?;

    let count = stream.read_u8().await?;
    if count == 0 {
        let reason = read_reason(stream).await?;
        return Err(RfbError::protocol(format!("a refusal: {reason}")));
    }
    let mut types = vec![0u8; usize::from(count)];
    stream.read_exact(&mut types).await?;
    if !types.contains(&1) {
        return Err(RfbError::protocol(format!(
            "security types {types:?}, none of them None (1)"
        )));
    }
    stream.write_all(&[1]).await?;
    if stream.read_u32().await? != 0 {
        let reason = read_reason(stream).await?;
        return Err(RfbError::protocol(format!("a refusal: {reason}")));
    }

    // ClientInit: share the desktop with any other viewer.
    stream.write_all(&[1]).await?;
    let width = stream.read_u16().await?;
    let height = stream.read_u16().await?;
    let mut _server_format = [0u8; 16];
    stream.read_exact(&mut _server_format).await?;
    let name_len = stream.read_u32().await?;
    if name_len > MAX_NAME {
        return Err(RfbError::protocol(format!(
            "a {name_len}-byte desktop name"
        )));
    }
    let mut name = vec![0u8; usize::try_from(name_len).expect("bounded above")];
    stream.read_exact(&mut name).await?;
    check_size(width, height)?;

    let mut setup = Vec::with_capacity(64);
    set_pixel_format(&mut setup);
    set_encodings(&mut setup);
    Outbound::Request {
        incremental: false,
        width,
        height,
    }
    .encode(&mut setup);
    stream.write_all(&setup).await?;
    stream.flush().await?;

    Ok(ServerInit {
        width,
        height,
        name: String::from_utf8_lossy(&name).into_owned(),
    })
}

async fn read_reason<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String, RfbError> {
    let len = stream.read_u32().await?.min(MAX_NAME);
    let mut reason = vec![0u8; usize::try_from(len).expect("bounded above")];
    stream.read_exact(&mut reason).await?;
    Ok(String::from_utf8_lossy(&reason).into_owned())
}

fn check_size(width: u16, height: u16) -> Result<(), RfbError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(RfbError::protocol(format!("a {width}x{height} desktop")));
    }
    Ok(())
}

/// SetPixelFormat: 32 bits per pixel, depth 24, little endian, true colour,
/// red at bit 16. Each pixel arrives as the `0x00RRGGBB` the window shows.
fn set_pixel_format(out: &mut Vec<u8>) {
    out.extend([0, 0, 0, 0]);
    out.extend([32, 24, 0, 1]);
    out.extend(255u16.to_be_bytes());
    out.extend(255u16.to_be_bytes());
    out.extend(255u16.to_be_bytes());
    out.extend([16, 8, 0, 0, 0, 0]);
}

fn set_encodings(out: &mut Vec<u8>) {
    out.extend([2, 0]);
    out.extend(
        u16::try_from(ENCODINGS.len())
            .expect("a handful")
            .to_be_bytes(),
    );
    for encoding in ENCODINGS {
        out.extend(encoding.to_be_bytes());
    }
}

/// A message from the window to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outbound {
    /// FramebufferUpdateRequest for the whole desktop.
    Request {
        incremental: bool,
        width: u16,
        height: u16,
    },
    /// PointerEvent: the buttons held (bit 0 left, 1 middle, 2 right, 3–6
    /// the wheel's four directions) and where the pointer is.
    Pointer { buttons: u8, x: u16, y: u16 },
    /// KeyEvent: an X11 keysym pressed or released.
    Key { down: bool, keysym: u32 },
}

impl Outbound {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Self::Request {
                incremental,
                width,
                height,
            } => {
                out.extend([3, u8::from(incremental), 0, 0, 0, 0]);
                out.extend(width.to_be_bytes());
                out.extend(height.to_be_bytes());
            }
            Self::Pointer { buttons, x, y } => {
                out.extend([5, buttons]);
                out.extend(x.to_be_bytes());
                out.extend(y.to_be_bytes());
            }
            Self::Key { down, keysym } => {
                out.extend([4, u8::from(down), 0, 0]);
                out.extend(keysym.to_be_bytes());
            }
        }
    }
}

/// One framebuffer update, applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Update {
    /// The desktop changed size during the update.
    pub(crate) resized: bool,
}

/// The read half of the conversation: decodes the server's messages into
/// the framebuffer it owns.
pub(crate) struct Reader<R> {
    stream: R,
    fb: Framebuffer,
    zrle: ZrleDecoder,
    /// Reused for each rect's bytes.
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    pub(crate) fn new(stream: R, init: &ServerInit) -> Self {
        Self {
            stream,
            fb: Framebuffer::new(init.width, init.height),
            zrle: ZrleDecoder::new(),
            buf: Vec::new(),
        }
    }

    pub(crate) fn framebuffer(&self) -> &Framebuffer {
        &self.fb
    }

    /// Reads messages until one framebuffer update has been applied.
    /// Bells, colour maps and cut text are read and dropped on the way.
    pub(crate) async fn next_update(&mut self) -> Result<Update, RfbError> {
        loop {
            match self.stream.read_u8().await? {
                0 => return self.framebuffer_update().await,
                // SetColourMapEntries: unused with a true-colour format.
                1 => {
                    let mut header = [0u8; 5];
                    self.stream.read_exact(&mut header).await?;
                    let count = u16::from_be_bytes([header[3], header[4]]);
                    self.skip(u64::from(count) * 6).await?;
                }
                // Bell.
                2 => {}
                // ServerCutText: the clipboard, not handled yet.
                3 => {
                    let mut pad = [0u8; 3];
                    self.stream.read_exact(&mut pad).await?;
                    let len = self.stream.read_u32().await?;
                    if len > MAX_CUT_TEXT {
                        return Err(RfbError::protocol(format!("{len} bytes of cut text")));
                    }
                    self.skip(u64::from(len)).await?;
                }
                other => {
                    return Err(RfbError::protocol(format!("message type {other}")));
                }
            }
        }
    }

    async fn skip(&mut self, n: u64) -> Result<(), RfbError> {
        let copied =
            tokio::io::copy(&mut (&mut self.stream).take(n), &mut tokio::io::sink()).await?;
        if copied < n {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        Ok(())
    }

    async fn framebuffer_update(&mut self) -> Result<Update, RfbError> {
        let _pad = self.stream.read_u8().await?;
        let rects = self.stream.read_u16().await?;
        let mut update = Update { resized: false };
        for _ in 0..rects {
            let rect = Rect {
                x: self.stream.read_u16().await?,
                y: self.stream.read_u16().await?,
                width: self.stream.read_u16().await?,
                height: self.stream.read_u16().await?,
            };
            match self.stream.read_i32().await? {
                ENCODING_RAW => self.raw(rect).await?,
                ENCODING_ZRLE => {
                    self.fb.check(rect)?;
                    let len = self.stream.read_u32().await?;
                    if len > MAX_ZRLE_BYTES {
                        return Err(RfbError::protocol(format!("a {len}-byte ZRLE rect")));
                    }
                    self.buf
                        .resize(usize::try_from(len).expect("bounded above"), 0);
                    self.stream.read_exact(&mut self.buf).await?;
                    self.zrle.decode_rect(&self.buf, rect, &mut self.fb)?;
                }
                ENCODING_DESKTOP_SIZE => {
                    check_size(rect.width, rect.height)?;
                    self.fb.resize(rect.width, rect.height);
                    update.resized = true;
                }
                ENCODING_EXTENDED_DESKTOP_SIZE => {
                    let screens = self.stream.read_u8().await?;
                    self.skip(3 + 16 * u64::from(screens)).await?;
                    // x is why the size changed and y whether a request of
                    // ours failed; a failure leaves the size as it was.
                    if rect.y == 0 {
                        check_size(rect.width, rect.height)?;
                        self.fb.resize(rect.width, rect.height);
                        update.resized = true;
                    }
                }
                other => {
                    return Err(RfbError::protocol(format!(
                        "a rect in encoding {other}, which this client did not ask for"
                    )));
                }
            }
        }
        Ok(update)
    }

    async fn raw(&mut self, rect: Rect) -> Result<(), RfbError> {
        self.fb.check(rect)?;
        let row_bytes = usize::from(rect.width) * 4;
        self.buf.resize(row_bytes, 0);
        for dy in 0..rect.height {
            self.stream.read_exact(&mut self.buf).await?;
            for (dx, px) in self.buf.as_chunks::<4>().0.iter().enumerate() {
                let rgb = u32::from_le_bytes(*px) & 0x00ff_ffff;
                let dx = u16::try_from(dx).expect("within the rect's width");
                self.fb.set(rect.x + dx, rect.y + dy, rgb);
            }
        }
        Ok(())
    }
}

/// Writes `messages` to the server, in order, as one write.
pub(crate) async fn send<W: AsyncWrite + Unpin>(
    stream: &mut W,
    messages: &[Outbound],
) -> Result<(), RfbError> {
    let mut out = Vec::with_capacity(messages.len() * 10);
    for message in messages {
        message.encode(&mut out);
    }
    stream.write_all(&out).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use tokio::io::DuplexStream;

    use super::*;

    /// The server's half of a handshake that accepts this client: version,
    /// security None, ServerInit for a `width`×`height` desktop named "box".
    pub(crate) fn server_hello(width: u16, height: u16) -> Vec<u8> {
        let mut hello = b"RFB 003.008\n".to_vec();
        hello.extend([1, 1]);
        hello.extend(0u32.to_be_bytes());
        hello.extend(width.to_be_bytes());
        hello.extend(height.to_be_bytes());
        hello.extend([0; 16]);
        hello.extend(3u32.to_be_bytes());
        hello.extend(b"box");
        hello
    }

    /// What this client sends during a handshake with a `width`×`height`
    /// desktop, after the version line.
    fn client_hello_after_version(width: u16, height: u16) -> Vec<u8> {
        let mut expected = vec![1, 1];
        set_pixel_format(&mut expected);
        set_encodings(&mut expected);
        Outbound::Request {
            incremental: false,
            width,
            height,
        }
        .encode(&mut expected);
        expected
    }

    async fn connected(width: u16, height: u16) -> (Reader<DuplexStream>, DuplexStream) {
        let (mut client, mut server) = tokio::io::duplex(1 << 20);
        server
            .write_all(&server_hello(width, height))
            .await
            .unwrap();
        let init = handshake(&mut client).await.unwrap();
        let mut sent = vec![0u8; 12 + client_hello_after_version(width, height).len()];
        server.read_exact(&mut sent).await.unwrap();
        (Reader::new(client, &init), server)
    }

    fn rect_header(x: u16, y: u16, w: u16, h: u16, encoding: i32) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [x, y, w, h] {
            out.extend(v.to_be_bytes());
        }
        out.extend(encoding.to_be_bytes());
        out
    }

    fn update(rects: u16, body: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0];
        out.extend(rects.to_be_bytes());
        out.extend(body);
        out
    }

    /// A Raw rect's pixels, each `0x00RRGGBB` as little-endian bytes.
    fn raw_pixels(rgb: &[u32]) -> Vec<u8> {
        rgb.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    #[tokio::test]
    async fn the_handshake_asks_for_the_format_and_encodings_this_client_decodes() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        server.write_all(&server_hello(640, 480)).await.unwrap();
        let init = handshake(&mut client).await.unwrap();
        assert_eq!(
            init,
            ServerInit {
                width: 640,
                height: 480,
                name: "box".into()
            }
        );

        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        let expected = client_hello_after_version(640, 480);
        let mut rest = vec![0u8; expected.len()];
        server.read_exact(&mut rest).await.unwrap();
        assert_eq!(rest, expected);
        // ZRLE before Raw: neatvnc takes the first frame encoding listed.
        assert_eq!(ENCODINGS[0], ENCODING_ZRLE);
        // The pixel format: 32 bpp, depth 24, little endian, true colour,
        // red/green/blue at bits 16/8/0.
        assert_eq!(
            &rest[2..22],
            &[
                0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0
            ]
        );
    }

    #[tokio::test]
    async fn a_server_older_than_3_8_is_refused() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        server.write_all(b"RFB 003.003\n").await.unwrap();
        let err = handshake(&mut client).await.unwrap_err();
        assert!(err.to_string().contains("RFB 3.8"), "{err}");
    }

    #[tokio::test]
    async fn a_refusal_before_security_carries_its_reason() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let mut refusal = b"RFB 003.008\n".to_vec();
        refusal.push(0);
        refusal.extend(9u32.to_be_bytes());
        refusal.extend(b"too many!");
        server.write_all(&refusal).await.unwrap();
        let err = handshake(&mut client).await.unwrap_err();
        assert!(err.to_string().contains("too many!"), "{err}");
    }

    #[tokio::test]
    async fn a_server_without_security_none_is_refused() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let mut hello = b"RFB 003.008\n".to_vec();
        hello.extend([1, 2]);
        server.write_all(&hello).await.unwrap();
        let err = handshake(&mut client).await.unwrap_err();
        assert!(err.to_string().contains("None"), "{err}");
    }

    #[tokio::test]
    async fn a_failed_security_result_carries_its_reason() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let mut hello = b"RFB 003.008\n".to_vec();
        hello.extend([1, 1]);
        hello.extend(1u32.to_be_bytes());
        hello.extend(4u32.to_be_bytes());
        hello.extend(b"nope");
        server.write_all(&hello).await.unwrap();
        let err = handshake(&mut client).await.unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[tokio::test]
    async fn an_oversized_desktop_is_refused() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        server
            .write_all(&server_hello(MAX_DIMENSION + 1, 10))
            .await
            .unwrap();
        assert!(handshake(&mut client).await.is_err());
    }

    #[tokio::test]
    async fn a_raw_rect_lands_in_the_framebuffer() {
        let (mut reader, mut server) = connected(4, 2).await;
        let mut body = rect_header(1, 0, 2, 2, ENCODING_RAW);
        body.extend(raw_pixels(&[0x11, 0x22, 0x33, 0xff_00_00_44]));
        server.write_all(&update(1, &body)).await.unwrap();
        let applied = reader.next_update().await.unwrap();
        assert!(!applied.resized);
        let fb = reader.framebuffer();
        assert_eq!([fb.get(1, 0), fb.get(2, 0)], [0x11, 0x22]);
        // The padding byte is dropped.
        assert_eq!([fb.get(1, 1), fb.get(2, 1)], [0x33, 0x44]);
        assert_eq!(fb.get(0, 0), 0);
    }

    /// wayvnc's resize: the new size and the pixels at that size in one
    /// update, applied in wire order (vnc-rs #22).
    #[tokio::test]
    async fn a_resize_applies_before_the_rects_that_follow_it() {
        let (mut reader, mut server) = connected(2, 2).await;
        let mut body = rect_header(0, 0, 4, 3, ENCODING_EXTENDED_DESKTOP_SIZE);
        body.extend([1, 0, 0, 0]);
        body.extend([0; 16]);
        body.extend(rect_header(3, 2, 1, 1, ENCODING_RAW));
        body.extend(raw_pixels(&[0xabcdef]));
        server.write_all(&update(2, &body)).await.unwrap();
        let applied = reader.next_update().await.unwrap();
        assert!(applied.resized);
        let fb = reader.framebuffer();
        assert_eq!((fb.width(), fb.height()), (4, 3));
        assert_eq!(fb.get(3, 2), 0xabcdef);
    }

    #[tokio::test]
    async fn a_failed_extended_desktop_size_keeps_the_size() {
        let (mut reader, mut server) = connected(2, 2).await;
        let mut body = rect_header(1, 3, 4, 3, ENCODING_EXTENDED_DESKTOP_SIZE);
        body.extend([0, 0, 0, 0]);
        server.write_all(&update(1, &body)).await.unwrap();
        assert!(!reader.next_update().await.unwrap().resized);
        assert_eq!(reader.framebuffer().width(), 2);
    }

    #[tokio::test]
    async fn desktop_size_keeps_the_overlapping_pixels() {
        let (mut reader, mut server) = connected(2, 1).await;
        let mut body = rect_header(0, 0, 2, 1, ENCODING_RAW);
        body.extend(raw_pixels(&[7, 8]));
        body.extend(rect_header(0, 0, 1, 2, ENCODING_DESKTOP_SIZE));
        server.write_all(&update(2, &body)).await.unwrap();
        reader.next_update().await.unwrap();
        let fb = reader.framebuffer();
        assert_eq!((fb.width(), fb.height()), (1, 2));
        assert_eq!([fb.get(0, 0), fb.get(0, 1)], [7, 0]);
    }

    #[tokio::test]
    async fn a_rect_outside_the_desktop_is_an_error() {
        let (mut reader, mut server) = connected(2, 2).await;
        let body = rect_header(1, 1, 2, 1, ENCODING_RAW);
        server.write_all(&update(1, &body)).await.unwrap();
        let err = reader.next_update().await.unwrap_err();
        assert!(err.to_string().contains("outside"), "{err}");
    }

    #[tokio::test]
    async fn an_encoding_not_asked_for_is_an_error() {
        let (mut reader, mut server) = connected(2, 2).await;
        let body = rect_header(0, 0, 1, 1, 7);
        server.write_all(&update(1, &body)).await.unwrap();
        assert!(reader.next_update().await.is_err());
    }

    #[tokio::test]
    async fn an_unknown_message_type_is_an_error() {
        let (mut reader, mut server) = connected(2, 2).await;
        server.write_all(&[42]).await.unwrap();
        let err = reader.next_update().await.unwrap_err();
        assert!(err.to_string().contains("message type 42"), "{err}");
    }

    #[tokio::test]
    async fn bells_colour_maps_and_cut_text_are_skipped() {
        let (mut reader, mut server) = connected(1, 1).await;
        let mut stream = vec![2];
        stream.extend([1, 0, 0, 0, 0, 2]);
        stream.extend([0; 12]);
        stream.extend([3, 0, 0, 0]);
        stream.extend(5u32.to_be_bytes());
        stream.extend(b"hello");
        let mut body = rect_header(0, 0, 1, 1, ENCODING_RAW);
        body.extend(raw_pixels(&[9]));
        stream.extend(update(1, &body));
        server.write_all(&stream).await.unwrap();
        reader.next_update().await.unwrap();
        assert_eq!(reader.framebuffer().get(0, 0), 9);
    }

    #[tokio::test]
    async fn a_stream_that_ends_mid_update_is_an_io_error() {
        let (mut reader, mut server) = connected(2, 2).await;
        server.write_all(&[0, 0, 0, 1, 0]).await.unwrap();
        drop(server);
        assert!(matches!(reader.next_update().await, Err(RfbError::Io(_))));
    }

    #[test]
    fn input_messages_have_the_rfb_layout() {
        let mut out = Vec::new();
        Outbound::Pointer {
            buttons: 0b101,
            x: 300,
            y: 2,
        }
        .encode(&mut out);
        Outbound::Key {
            down: true,
            keysym: 0xffeb,
        }
        .encode(&mut out);
        Outbound::Request {
            incremental: true,
            width: 1280,
            height: 720,
        }
        .encode(&mut out);
        assert_eq!(
            out,
            [
                5, 5, 1, 44, 0, 2, // pointer
                4, 1, 0, 0, 0, 0, 0xff, 0xeb, // key
                3, 1, 0, 0, 0, 0, 5, 0, 2, 208, // request
            ]
        );
    }
}

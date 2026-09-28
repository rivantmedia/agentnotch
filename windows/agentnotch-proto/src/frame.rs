//! Framing. A pipe has no half-close, so every message in either direction
//! is `u32` little-endian byte length + that many bytes of UTF-8 JSON (one
//! object). "No frame, then EOF" means "no decision".

use std::io::{self, Read, Write};

/// Why no frame was read. Not serialisable: it never leaves the process.
#[derive(Debug)]
pub enum FrameError {
    /// The peer closed before sending a single byte: no frame.
    Eof,
    /// The declared length is above the reader's limit; nothing more was read.
    TooLarge(usize),
    /// A read failed, or the peer closed in the middle of a frame.
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Eof => write!(f, "the peer closed without sending a frame"),
            FrameError::TooLarge(len) => write!(f, "a frame of {len} bytes is over the limit"),
            FrameError::Io(error) => write!(f, "reading a frame failed: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// The frame for `json`: its length, then its bytes.
pub fn encode_frame(json: &[u8]) -> io::Result<Vec<u8>> {
    let len = u32::try_from(json.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a frame can't be over 4 GiB"))?;
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(json);
    Ok(frame)
}

/// Writes one frame in a single `write_all`, so a message pipe reader never
/// sees a length without its body sent in the same write.
pub fn write_frame(w: &mut impl Write, json: &[u8]) -> io::Result<()> {
    let frame = encode_frame(json)?;
    w.write_all(&frame)?;
    w.flush()
}

/// Reads one frame of at most `max` bytes.
pub fn read_frame(r: &mut impl Read, max: usize) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < header.len() {
        match r.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Err(FrameError::Eof),
            Ok(0) => return Err(FrameError::Io(io::ErrorKind::UnexpectedEof.into())),
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    let len = u32::from_le_bytes(header) as usize;
    if len > max {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(FrameError::Io)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, br#"{"event":"Stop"}"#).unwrap();
        assert_eq!(&buffer[..4], &16u32.to_le_bytes());
        let mut reader = Cursor::new(buffer);
        assert_eq!(
            read_frame(&mut reader, 1024).unwrap(),
            br#"{"event":"Stop"}"#
        );
        assert!(matches!(
            read_frame(&mut reader, 1024),
            Err(FrameError::Eof)
        ));
    }

    #[test]
    fn two_frames_back_to_back() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"{}").unwrap();
        write_frame(&mut buffer, b"[1]").unwrap();
        let mut reader = Cursor::new(buffer);
        assert_eq!(read_frame(&mut reader, 16).unwrap(), b"{}");
        assert_eq!(read_frame(&mut reader, 16).unwrap(), b"[1]");
    }

    #[test]
    fn empty_body_is_a_frame() {
        let mut reader = Cursor::new(0u32.to_le_bytes().to_vec());
        assert_eq!(read_frame(&mut reader, 0).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_big_frame_is_refused_before_it_is_read() {
        let mut reader = Cursor::new((9u32 << 20).to_le_bytes().to_vec());
        assert!(
            matches!(read_frame(&mut reader, 8 << 20), Err(FrameError::TooLarge(len)) if len == 9 << 20)
        );
    }

    #[test]
    fn a_cut_frame_is_an_error_not_a_frame() {
        let mut short_header = Cursor::new(vec![5u8, 0]);
        assert!(matches!(
            read_frame(&mut short_header, 64),
            Err(FrameError::Io(_))
        ));
        let mut short_body = Cursor::new([&5u32.to_le_bytes()[..], b"ab"].concat());
        assert!(matches!(
            read_frame(&mut short_body, 64),
            Err(FrameError::Io(_))
        ));
    }

    /// A reader that hands out one byte at a time, like a pipe under load.
    struct Trickle(Cursor<Vec<u8>>);
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let end = buf.len().min(1);
            self.0.read(&mut buf[..end])
        }
    }

    #[test]
    fn partial_reads_are_joined() {
        let frame = encode_frame(br#"{"a":1}"#).unwrap();
        let mut reader = Trickle(Cursor::new(frame));
        assert_eq!(read_frame(&mut reader, 64).unwrap(), br#"{"a":1}"#);
    }
}

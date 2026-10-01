//! The COM plumbing behind the sealed snapshots (DESIGN-WIN §7.4; WP9).
//!
//! WebView2's `CapturePreview` itself is called by the app's glue, where `webview2-com` lives
//! (seam WB); what it needs besides is here: an in-memory `IStream` for the PNG to be written
//! into, and the bytes read back out of it. The glue also needs to name a few items of the
//! `windows` version WebView2's bindings are built on (0.61, the one this crate uses; the app
//! crate's own `windows` dependency is upstream's older one), so they are re-exported below
//! rather than adding a second `windows` line to upstream's manifest.

use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::SHCreateMemStream;

/// `cast` between WebView2 interfaces (e.g. `ICoreWebView2Settings` → `…Settings3`).
pub use windows::core::Interface as ComInterface;
/// The stream `CapturePreview` writes into.
pub use windows::Win32::System::Com::IStream;

/// A capture larger than this is refused: a page is a few hundred kilobytes as a PNG, and the
/// stream's own size is what the read below trusts.
const MAX_CAPTURE: u64 = 64 << 20;

/// An empty stream in memory.
pub fn memory_stream() -> Result<IStream, String> {
    // SAFETY: no initial data; the returned stream owns its memory.
    unsafe { SHCreateMemStream(None) }.ok_or_else(|| "no memory for the capture".to_string())
}

/// Everything written into `stream`, from its start.
pub fn stream_bytes(stream: &IStream) -> Result<Vec<u8>, String> {
    let mut stat = STATSTG::default();
    // SAFETY: `stat` is a valid out pointer; STATFLAG_NONAME means no name is allocated in it.
    unsafe { stream.Stat(&mut stat, STATFLAG_NONAME) }.map_err(|e| e.message())?;
    let size = stat.cbSize;
    if size > MAX_CAPTURE {
        return Err(format!("the capture is too large ({size} bytes)"));
    }
    // SAFETY: repositions the stream; no pointer arguments besides the optional new position.
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.map_err(|e| e.message())?;
    let mut bytes = vec![0u8; size as usize];
    let mut filled = 0usize;
    while filled < bytes.len() {
        let mut read = 0u32;
        let want = (bytes.len() - filled).min(u32::MAX as usize) as u32;
        // SAFETY: the destination has at least `want` bytes left from `filled`; `read` is a valid
        // out pointer.
        let status =
            unsafe { stream.Read(bytes[filled..].as_mut_ptr().cast(), want, Some(&mut read)) };
        status.ok().map_err(|e| e.message())?;
        if read == 0 {
            break;
        }
        filled += read as usize;
    }
    bytes.truncate(filled);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_into_a_memory_stream_is_read_back_whole() {
        let stream = memory_stream().expect("a stream");
        // Longer than one read is ever likely to be served in, and not a repeating page.
        let bytes: Vec<u8> = (0..70_000u32).map(|n| (n % 251) as u8).collect();
        let mut written = 0u32;
        // SAFETY: the source holds `bytes.len()` readable bytes; `written` is a valid out pointer.
        unsafe {
            stream.Write(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                Some(&mut written),
            )
        }
        .ok()
        .expect("the stream takes the bytes");
        assert_eq!(written as usize, bytes.len());
        assert_eq!(stream_bytes(&stream).expect("read back"), bytes);
        // Reading leaves the stream as it was: a second read gives the same bytes.
        assert_eq!(stream_bytes(&stream).expect("read again"), bytes);
        let empty = memory_stream().expect("a stream");
        assert_eq!(stream_bytes(&empty).expect("read"), Vec::<u8>::new());
    }
}

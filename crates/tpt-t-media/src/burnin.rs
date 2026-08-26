//! Telemetry "burn-in" — rasterizing an on-screen HUD onto a video frame
//! *before* it is handed to the encoder (spec §5.3 "telemetry burn-in onto
//! video frame pre-encode").
//!
//! The HUD is drawn with a built-in 5×7 bitmap font ([`GLYPHS`]) directly into
//! the caller's pixel buffer: zero intermediate allocation, zero copy. The
//! same pixel-writing routine handles RGB888 (sets all three channels to the
//! shade), GrayY8 (luma), and the NV12 luma plane (chroma left untouched,
//! which is acceptable for a thin top strip). Unknown glyphs are skipped so a
//! label is never partially clobbered.
//!
//! A real font asset (or the vendor HUD glyph pack) can replace [`GLYPHS`]
//! without touching the drawing code.

use core::fmt::Write;

use tpt_t_core::ser::{TelemetryKind, TelemetryPacket};

use crate::pool::{FrameMeta, PixFmt};

/// Glyph width in pixels.
pub const GLYPH_W: u32 = 5;
/// Glyph height in pixels.
pub const GLYPH_H: u32 = 7;
/// Horizontal advance (glyph + 1px gap).
pub const GLYPH_ADVANCE: u32 = GLYPH_W + 1;

/// One 5×7 glyph: 7 rows, each byte's low 5 bits are columns (bit 4 = left).
type Glyph = [u8; 7];

/// Built-in font covering `0-9`, `A-Z`, space, and a few punctuation marks
/// used by telemetry labels. Any other code point renders as nothing.
const GLYPHS: &[(u8, Glyph)] = &[
    (b' ', [0x00; 7]),
    (b'0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    (b'1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    (b'2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    (b'3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    (b'4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    (b'5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    (b'6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    (b'7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    (b'8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    (b'9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    (b'A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    (b'B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    (b'C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    (b'D', [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E]),
    (b'E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    (b'F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    (b'G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    (b'H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    (b'I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    (b'J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    (b'K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    (b'L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    (b'M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    (b'N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    (b'O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    (b'P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    (b'Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    (b'R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    (b'S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    (b'T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    (b'U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    (b'V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    (b'W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    (b'X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    (b'Y', [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04]),
    (b'Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    (b':', [0x04, 0x04, 0x00, 0x04, 0x04, 0x00, 0x04]),
    (b'.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C]),
    (b'-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    (b'%', [0x19, 0x1A, 0x04, 0x0B, 0x13, 0x00, 0x00]),
    (b'/', [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10]),
    (b',', [0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C, 0x04]),
];

/// Looks up the glyph for `ch`, or `None` when the font lacks it.
#[inline]
pub fn glyph(ch: u8) -> Option<Glyph> {
    GLYPHS.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g)
}

/// Writes one pixel at `(x, y)` in the frame's pixel format. Out-of-range
/// coordinates are ignored (callers never need to clamp by hand).
#[inline]
fn set_pixel(frame: &mut [u8], meta: &FrameMeta, x: i32, y: i32, shade: u8) {
    let w = meta.width as i32;
    let h = meta.height as i32;
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let (x, y) = (x as usize, y as usize);
    match meta.pixfmt() {
        PixFmt::GrayY8 | PixFmt::Nv12 => {
            // NV12: write the luma plane; chroma is left intact.
            let stride = meta.stride as usize;
            frame[y * stride + x] = shade;
        }
        PixFmt::Rgb888 => {
            let stride = meta.stride as usize;
            let base = y * stride + x * 3;
            frame[base..base + 3].copy_from_slice(&[shade, shade, shade]);
        }
    }
}

/// Draws `ch` at pixel `(x, y)` (top-left of the glyph). Returns the x
/// advance for the next character.
#[inline]
pub fn draw_glyph(frame: &mut [u8], meta: &FrameMeta, ch: u8, x: i32, y: i32, shade: u8) -> i32 {
    if let Some(g) = glyph(ch) {
        for (row, bits) in g.iter().enumerate() {
            for col in 0..GLYPH_W as i32 {
                if (bits >> (4 - col)) & 1 != 0 {
                    set_pixel(frame, meta, x + col, y + row as i32, shade);
                }
            }
        }
    }
    GLYPH_ADVANCE as i32
}

/// Rasterizes `text` into the frame starting at `(x, y)`. Long strings are
/// clipped at the right edge rather than wrapped or panicking.
pub fn burn_in_text(frame: &mut [u8], meta: &FrameMeta, text: &str, x: i32, y: i32, shade: u8) {
    let mut cx = x;
    for &b in text.as_bytes() {
        cx += draw_glyph(frame, meta, b, cx, y, shade);
        if cx > meta.width as i32 {
            break;
        }
    }
}

/// Fixed-capacity string buffer for formatting telemetry without allocation.
struct BufWriter {
    buf: [u8; 128],
    len: usize,
}

impl BufWriter {
    fn new() -> Self {
        Self {
            buf: [0u8; 128],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        // The buffer only ever holds ASCII written by `write_str` below.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for BufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let free = self.buf.len() - self.len;
        let take = s.len().min(free);
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Formats a telemetry packet into a compact HUD line, e.g.
/// `BAT 87.0 %` or `GEN 1.0 2.0 3.0 ...`.
fn format_telemetry(pkt: &TelemetryPacket, out: &mut BufWriter) {
    let _ = match TelemetryKind::from_u16(pkt.kind) {
        Some(TelemetryKind::Battery) => write!(out, "BAT "),
        Some(TelemetryKind::Pose) => write!(out, "POS "),
        Some(TelemetryKind::Temperature) => write!(out, "TMP "),
        _ => write!(out, "GEN "),
    };
    for v in pkt.values.iter() {
        let _ = write!(out, "{:.1} ", v);
    }
}

/// Convenience: draws a telemetry packet's HUD line into the top-left strip
/// of `frame` (a bright shade works for both grayscale and RGB).
pub fn burn_in_telemetry(frame: &mut [u8], meta: &FrameMeta, pkt: &TelemetryPacket, shade: u8) {
    let mut buf = BufWriter::new();
    format_telemetry(pkt, &mut buf);
    burn_in_text(frame, meta, buf.as_str(), 2, 2, shade);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::ser::TelemetryPacket;

    fn rgb_meta(w: u32, h: u32) -> FrameMeta {
        FrameMeta::new(0, 0, w, h, PixFmt::Rgb888)
    }

    fn lit_pixels(frame: &[u8], meta: &FrameMeta) -> usize {
        let stride = meta.stride as usize;
        match meta.pixfmt() {
            PixFmt::Rgb888 => frame
                .chunks(stride)
                .take(meta.height as usize)
                .flat_map(|row| row.chunks(3))
                .filter(|px| px[0] != 0)
                .count(),
            _ => frame.iter().filter(|&&b| b != 0).count(),
        }
    }

    #[test]
    fn glyph_table_is_complete_and_well_shaped() {
        assert!(glyph(b'A').is_some());
        assert!(glyph(b'Z').is_some());
        assert!(glyph(b'9').is_some());
        assert!(glyph(b' ').is_some());
        assert!(glyph(b'~').is_none(), "unknown glyph absent");
        // 'B' must differ from '8'.
        assert_ne!(glyph(b'B').unwrap(), glyph(b'8').unwrap());
    }

    #[test]
    fn burn_in_lights_pixels_in_top_strip() {
        let meta = rgb_meta(64, 32);
        let mut frame = vec![0u8; PixFmt::Rgb888.min_buffer_len(64, 32)];
        let before = lit_pixels(&frame, &meta);
        let pkt = TelemetryPacket {
            values: [87.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..TelemetryPacket::zeroed(TelemetryKind::Battery, 1, 2)
        };
        burn_in_telemetry(&mut frame, &meta, &pkt, 255);
        let after = lit_pixels(&frame, &meta);
        assert_eq!(before, 0);
        assert!(after > 100, "HUD must paint many pixels: {after}");
    }

    #[test]
    fn different_packets_produce_different_overlays() {
        let meta = rgb_meta(128, 32);
        let mk = |v: f32| TelemetryPacket {
            values: [v, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..TelemetryPacket::zeroed(TelemetryKind::Generic, 1, 2)
        };
        let mut a = vec![0u8; PixFmt::Rgb888.min_buffer_len(128, 32)];
        let mut b = vec![0u8; PixFmt::Rgb888.min_buffer_len(128, 32)];
        burn_in_telemetry(&mut a, &meta, &mk(1.0), 255);
        burn_in_telemetry(&mut b, &meta, &mk(2.0), 255);
        assert_ne!(a, b, "distinct values must yield distinct HUD");
    }

    #[test]
    fn out_of_bounds_text_is_clipped_not_panicking() {
        let meta = rgb_meta(20, 10);
        let mut frame = vec![0u8; PixFmt::Rgb888.min_buffer_len(20, 10)];
        // "WWWWWWWW" advances well past width 20.
        burn_in_text(&mut frame, &meta, "WWWWWWWW", 0, 0, 200);
        // Still must have painted the in-bounds portion.
        assert!(lit_pixels(&frame, &meta) > 0);
    }

    #[test]
    fn nv12_burn_in_writes_luma_only() {
        let meta = FrameMeta::new(0, 0, 32, 16, PixFmt::Nv12);
        let mut frame = vec![0u8; PixFmt::Nv12.min_buffer_len(32, 16)];
        let y_plane = 32 * 16;
        let chroma_before = frame[y_plane..].to_vec();
        burn_in_text(&mut frame, &meta, "A1", 0, 0, 255);
        // Chroma plane untouched.
        assert_eq!(&frame[y_plane..], &chroma_before[..]);
        // Some luma lit.
        assert!(frame[..y_plane].iter().any(|&b| b != 0));
    }
}

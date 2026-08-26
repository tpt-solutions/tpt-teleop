//! CRC32 (IEEE 802.3 reflected, poly `0xEDB88320`) — dependency-free.
//!
//! Used at two layers of the link framing (spec §5.2):
//!
//! * Frame trailer: every datagram ends with a CRC32 covering the header and
//!   payload bytes, so corruption is detected before any parse work.
//! * [`tpt_t_core::ser::ControlCommand`]'s `crc` field: filled by
//!   [`command_crc`] over the struct's own raw image with the field zeroed,
//!   giving shared-memory/raw-cast consumers the same integrity check.

/// Precomputed half-byte-free lookup table (const-evaluated into `.rodata`;
/// no runtime init, no lazy static).
static TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// CRC32 of `data` (standard init/final XOR with `!0`).
#[inline]
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// Integrity tag for a [`ControlCommand`]: CRC32 over its 56-byte raw image
/// with the `crc` field itself zeroed. Sender and receiver both compute this
/// from the decoded value, so it survives rkyv round-trips unchanged.
pub fn command_crc(cmd: &tpt_t_core::ser::ControlCommand) -> u32 {
    let mut tmp = *cmd;
    tmp.crc = 0;
    crc32(tpt_t_ring::cast::bytes_of(&tmp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::mode::Mode;
    use tpt_t_core::ser::ControlCommand;

    #[test]
    fn matches_known_check_vectors() {
        // Canonical IEEE CRC-32 check values.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn command_crc_is_stable_and_field_sensitive() {
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.seq = 42;
        let a = command_crc(&cmd);
        assert_ne!(a, 0);
        assert_eq!(command_crc(&cmd), a, "deterministic");

        cmd.axes[3] = 0.5;
        assert_ne!(command_crc(&cmd), a, "payload changes must change crc");

        // The crc field itself must not feed the digest.
        let mut tagged = cmd;
        tagged.crc = command_crc(&cmd);
        assert_eq!(command_crc(&tagged), tagged.crc);
    }
}

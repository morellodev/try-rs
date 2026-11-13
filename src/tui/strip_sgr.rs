//! `--no-colors` writer: removes SGR sequences (`ESC [ … m`) and keeps the
//! cursor and erase sequences that in-place frames need.

use std::io::{self, Write};

const ESC: u8 = 0x1b;
const CSI_FINAL: std::ops::RangeInclusive<u8> = 0x40..=0x7e;

#[derive(Debug)]
pub struct StripSgr<W> {
    inner: W,
    /// Incomplete escape sequence; it can span `write` calls.
    pending: Vec<u8>,
}

impl<W: Write> StripSgr<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }
}

impl<W: Write> Write for StripSgr<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut kept = Vec::with_capacity(buf.len());
        for &byte in buf {
            if self.pending.is_empty() {
                if byte == ESC {
                    self.pending.push(byte);
                } else {
                    kept.push(byte);
                }
                continue;
            }
            self.pending.push(byte);
            if self.pending.len() == 2 {
                if byte != b'[' {
                    kept.append(&mut self.pending);
                }
            } else if CSI_FINAL.contains(&byte) {
                if byte != b'm' {
                    kept.extend_from_slice(&self.pending);
                }
                self.pending.clear();
            }
        }
        self.inner.write_all(&kept)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> Vec<u8> {
        let mut out = StripSgr::new(Vec::new());
        for chunk in chunks {
            out.write_all(chunk).unwrap();
        }
        out.inner
    }

    #[test]
    fn removes_colors_and_attributes() {
        assert_eq!(
            strip(&[b"\x1b[38;5;245mdim\x1b[0m \x1b[1mbold\x1b[m"]),
            b"dim bold"
        );
    }

    #[test]
    fn keeps_cursor_erase_and_mode_sequences() {
        let input = b"\x1b[?2026h\x1b[H\x1b[K\x1b[5;1H\x1b[J\x1b[?25l\x1b[?2026l";
        assert_eq!(strip(&[input]), input);
    }

    #[test]
    fn handles_a_sequence_split_across_writes() {
        assert_eq!(strip(&[b"a\x1b[3", b"1mb\x1b", b"[2;1Hc"]), b"ab\x1b[2;1Hc");
    }

    #[test]
    fn keeps_non_csi_escapes() {
        assert_eq!(strip(&[b"\x1b7x\x1b8"]), b"\x1b7x\x1b8");
    }
}

//! Currently only support utf8 mode

#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

#[cfg(feature = "std")]
use std::io::{self, Write};

const UTF8_START: &[u8] = &[0x1B, 0x25, 0x47];
const UTF8_END: &[u8] = &[0x1B, 0x25, 0x40];

/// Wrapper for reduce allocation
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct CText<'s> {
    utf8: &'s str,
}

impl<'s> fmt::Debug for CText<'s> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.utf8)
    }
}

impl<'s> fmt::Display for CText<'s> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.utf8)
    }
}

impl<'s> CText<'s> {
    pub const fn new(utf8: &'s str) -> Self {
        Self { utf8 }
    }

    pub const fn len(self) -> usize {
        self.utf8.len() + UTF8_START.len() + UTF8_END.len()
    }

    #[cfg(feature = "std")]
    pub fn write(self, mut out: impl Write) -> io::Result<usize> {
        let mut writed = 0;
        writed += out.write(UTF8_START)?;
        writed += out.write(self.utf8.as_bytes())?;
        writed += out.write(UTF8_END)?;
        Ok(writed)
    }
}

/// Encoding utf8 to COMPOUND_TEXT with utf8 escape
pub fn utf8_to_compound_text(text: &str) -> Vec<u8> {
    let mut ret = Vec::with_capacity(text.len() + 6);
    ret.extend_from_slice(UTF8_START);
    ret.extend_from_slice(text.as_bytes());
    ret.extend_from_slice(UTF8_END);
    ret
}

#[derive(Debug, Clone)]
pub enum DecodeError {
    InvalidEncoding,
    UnsupportedEncoding,
    Utf8Error(alloc::string::FromUtf8Error),
}

impl From<alloc::string::FromUtf8Error> for DecodeError {
    fn from(err: alloc::string::FromUtf8Error) -> Self {
        DecodeError::Utf8Error(err)
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEncoding => write!(f, "Invalid compound text"),
            Self::UnsupportedEncoding => write!(f, "This encoding is not supported yet"),
            Self::Utf8Error(e) => write!(f, "Not a valid utf8 {}", e),
        }
    }
}

/// Decode COMPOUND_TEXT to UTF-8.
///
/// Patched for Lumen: this never fails. The original returned an error for
/// plain Latin-1 text (what ibus sends for "é") and for Chinese and Korean
/// charsets, and the XIM client turns that error into a panic. Here text
/// with no escapes is ASCII plus Latin-1 (compound text's default), UTF-8,
/// JIS X 0208, GB 2312 and KS C 5601 segments are decoded, and anything
/// else is skipped.
pub fn compound_text_to_utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Set {
        Ascii,
        Latin1,
        Utf8,
        /// A 94x94 set, decoded with an EUC encoding (bytes with the top bit set).
        Double(&'static encoding_rs::Encoding),
        Unknown,
    }
    let (mut gl, mut gr) = (Set::Ascii, Set::Latin1);
    let mut out = String::with_capacity(bytes.len());
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_set = Set::Ascii;
    let flush = |out: &mut String, pending: &mut Vec<u8>, set: Set| {
        match set {
            Set::Utf8 => out.push_str(&String::from_utf8_lossy(pending)),
            Set::Double(encoding) => {
                let high: Vec<u8> = pending.iter().map(|b| b | 0x80).collect();
                out.push_str(&encoding.decode_without_bom_handling(&high).0);
            }
            Set::Ascii | Set::Latin1 | Set::Unknown => {}
        }
        pending.clear();
    };
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1B {
            flush(&mut out, &mut pending, pending_set);
            // ESC, intermediate bytes (0x20..=0x2F), then a final byte.
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (0x20..=0x2F).contains(&bytes[end]) {
                end += 1;
            }
            let (inter, fin) = (&bytes[start..end], bytes.get(end).copied());
            i = end + 1;
            let double = |f| match f {
                Some(0x42) => Set::Double(encoding_rs::EUC_JP),
                Some(0x41) => Set::Double(encoding_rs::GBK),
                Some(0x43) => Set::Double(encoding_rs::EUC_KR),
                _ => Set::Unknown,
            };
            match (inter, fin) {
                (b"%", Some(0x47)) => gl = Set::Utf8,
                (b"%", Some(0x40)) => {
                    gl = Set::Ascii;
                    gr = Set::Latin1;
                }
                (b"(", Some(0x42)) => gl = Set::Ascii,
                (b"-", Some(0x41)) => gr = Set::Latin1,
                (b"$(", f) => gl = double(f),
                (b"$)", f) => gr = double(f),
                (b"(", _) => gl = Set::Unknown,
                (b"-", _) | (b")", _) => gr = Set::Unknown,
                _ => {}
            }
            continue;
        }
        i += 1;
        let set = if gl == Set::Utf8 {
            Set::Utf8
        } else if b < 0x80 {
            gl
        } else {
            gr
        };
        match set {
            Set::Ascii => out.push(b as char),
            Set::Latin1 => out.push(char::from(b)),
            Set::Utf8 | Set::Double(_) => {
                if set != pending_set {
                    flush(&mut out, &mut pending, pending_set);
                    pending_set = set;
                }
                pending.push(b & if set == Set::Utf8 { 0xFF } else { 0x7F });
            }
            Set::Unknown => {}
        }
        if !matches!(set, Set::Utf8 | Set::Double(_)) {
            flush(&mut out, &mut pending, pending_set);
        }
    }
    flush(&mut out, &mut pending, pending_set);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn korean() {
        const UTF8: &str = "가나다";
        const COMP: &[u8] = &[
            27, 37, 71, 234, 176, 128, 235, 130, 152, 235, 139, 164, 27, 37, 64,
        ];
        assert_eq!(crate::utf8_to_compound_text(UTF8), COMP);
        assert_eq!(crate::compound_text_to_utf8(COMP).unwrap(), UTF8);
    }

    #[test]
    fn latin1_without_escapes() {
        // What ibus sends for "é" (a bare 0xE9 crashed Lumen before).
        assert_eq!(crate::compound_text_to_utf8(&[0xE9]).unwrap(), "é");
        assert_eq!(crate::compound_text_to_utf8(b"caf\xe9").unwrap(), "café");
    }

    #[test]
    fn chinese_and_korean() {
        // GB 2312 "中文" and KS C 5601 "한", designated into GL.
        assert_eq!(crate::compound_text_to_utf8(&[27, 36, 40, 65, 0x56, 0x50, 0x4E, 0x44]).unwrap(), "中文");
        assert_eq!(crate::compound_text_to_utf8(&[27, 36, 40, 67, 0x47, 0x51]).unwrap(), "한");
    }

    #[test]
    fn mixed_and_unknown_never_fail() {
        let mut bytes = b"a".to_vec();
        bytes.extend_from_slice(&[27, 37, 71]);
        bytes.extend_from_slice("ü日".as_bytes());
        bytes.extend_from_slice(&[27, 37, 64, b'z', 27, 40, 74, b'q', 27, 40, 66, b'!']);
        assert_eq!(crate::compound_text_to_utf8(&bytes).unwrap(), "aü日z!");
        assert!(crate::compound_text_to_utf8(&[27]).is_ok());
    }

    #[test]
    fn iso_2011_jp() {
        const UTF8: &str = "東京";
        const COMP: &[u8] = &[27, 36, 40, 66, 69, 108, 53, 126];
        assert_eq!(crate::compound_text_to_utf8(COMP).unwrap(), UTF8);
    }
}

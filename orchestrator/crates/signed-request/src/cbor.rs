//! The few CBOR items the claim object is made of, written and read in the form RFC 8949 §4.2.1 calls deterministic:
//! definite lengths, and the shortest head for every number.

pub(crate) const MAJOR_UINT: u8 = 0;
pub(crate) const MAJOR_NINT: u8 = 1;
pub(crate) const MAJOR_BSTR: u8 = 2;
pub(crate) const MAJOR_TSTR: u8 = 3;
pub(crate) const MAJOR_ARRAY: u8 = 4;
pub(crate) const MAJOR_MAP: u8 = 5;
pub(crate) const MAJOR_TAG: u8 = 6;

/// The empty map: the unprotected header, which is always empty.
pub(crate) const EMPTY_MAP: u8 = 0xa0;

/// Writes the shortest head for `major` and `arg`.
pub(crate) fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let initial = major << 5;
    if let Ok(byte) = u8::try_from(arg) {
        if byte < 24 {
            out.push(initial | byte);
        } else {
            out.extend_from_slice(&[initial | 24, byte]);
        }
    } else if let Ok(short) = u16::try_from(arg) {
        out.push(initial | 25);
        out.extend_from_slice(&short.to_be_bytes());
    } else if let Ok(word) = u32::try_from(arg) {
        out.push(initial | 26);
        out.extend_from_slice(&word.to_be_bytes());
    } else {
        out.push(initial | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}

/// A length as a CBOR argument. Infallible on every target Rust supports; spelled out so no cast truncates.
pub(crate) fn len_arg(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

pub(crate) fn bstr(out: &mut Vec<u8>, bytes: &[u8]) {
    head(out, MAJOR_BSTR, len_arg(bytes.len()));
    out.extend_from_slice(bytes);
}

pub(crate) fn tstr(out: &mut Vec<u8>, text: &str) {
    head(out, MAJOR_TSTR, len_arg(text.len()));
    out.extend_from_slice(text.as_bytes());
}

/// A negative integer `-1 - n`.
pub(crate) fn nint(out: &mut Vec<u8>, n: u64) {
    head(out, MAJOR_NINT, n);
}

/// A reader of the items above that accepts only the deterministic form: a head that is not the shortest, an indefinite
/// length or a length past the end is an error.
#[cfg(feature = "verify")]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// Why a read failed. It says nothing about the bytes.
#[cfg(feature = "verify")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reject;

#[cfg(feature = "verify")]
impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    pub(crate) fn is_at_end(&self) -> bool {
        self.at == self.bytes.len()
    }

    /// The next head, as `(major, argument)`, in its shortest form only.
    fn head(&mut self) -> Result<(u8, u64), Reject> {
        let first = *self.bytes.get(self.at).ok_or(Reject)?;
        self.at += 1;
        let major = first >> 5;
        let info = first & 0x1f;
        let width = match info {
            0..=23 => return Ok((major, u64::from(info))),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(Reject),
        };
        let raw = self.bytes.get(self.at..self.at + width).ok_or(Reject)?;
        self.at += width;
        let arg = raw.iter().fold(0_u64, |acc, b| (acc << 8) | u64::from(*b));
        // not the shortest head: 24 for a value under 24, 25 for one that fits a byte, and so on
        let is_shortest = match width {
            1 => arg >= 24,
            2 => arg > 0xff,
            4 => arg > 0xffff,
            _ => arg > 0xffff_ffff,
        };
        if is_shortest {
            Ok((major, arg))
        } else {
            Err(Reject)
        }
    }

    /// A head of `major`; its argument.
    pub(crate) fn expect(&mut self, major: u8) -> Result<u64, Reject> {
        match self.head()? {
            (m, arg) if m == major => Ok(arg),
            _ => Err(Reject),
        }
    }

    pub(crate) fn uint(&mut self) -> Result<u64, Reject> {
        self.expect(MAJOR_UINT)
    }

    /// A negative integer, as `n` of `-1 - n`.
    pub(crate) fn nint(&mut self) -> Result<u64, Reject> {
        self.expect(MAJOR_NINT)
    }

    pub(crate) fn bstr(&mut self) -> Result<&'a [u8], Reject> {
        let len = usize::try_from(self.expect(MAJOR_BSTR)?).map_err(|_| Reject)?;
        let bytes = self
            .bytes
            .get(self.at..self.at.checked_add(len).ok_or(Reject)?);
        let bytes = bytes.ok_or(Reject)?;
        self.at += len;
        Ok(bytes)
    }

    pub(crate) fn tstr(&mut self) -> Result<&'a str, Reject> {
        let len = usize::try_from(self.expect(MAJOR_TSTR)?).map_err(|_| Reject)?;
        let bytes = self
            .bytes
            .get(self.at..self.at.checked_add(len).ok_or(Reject)?);
        let bytes = bytes.ok_or(Reject)?;
        self.at += len;
        std::str::from_utf8(bytes).map_err(|_| Reject)
    }

    /// One byte that must be `byte` (the empty map of the unprotected header).
    pub(crate) fn exact(&mut self, byte: u8) -> Result<(), Reject> {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(Reject)
        }
    }
}

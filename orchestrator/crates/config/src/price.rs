//! A price as the file writes it: a decimal string, read exactly (ADR 0065, decision 5).
//!
//! No floating point is involved anywhere: not in the parser of the file (a YAML number where a
//! string goes is a shape error) and not here. A price is digits, then optionally a point and one
//! to six more digits, and it becomes a whole number of **millionths** of the currency unit. The
//! errors say what a price looks like and never quote the text.

/// The dearest price: 1 000 000 currency units, in millionths. The same bound as the core's
/// `MAX_PRICE_MICROS`, which refuses a table above it again.
pub const MAX_PRICE_MICROS: u64 = 1_000_000_000_000;

/// The longest text a price may be before it is read at all.
const MAX_PRICE_BYTES: usize = 32;

/// Why a text is not a price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceError {
    /// Not digits with an optional point and fraction (a sign, an exponent, a space, a comma, a
    /// point with nothing before or after it, a letter or an empty text).
    #[error(
        "expected a decimal string: digits, then optionally a point and one to six more digits, \
         like \"0.40\" (no sign, exponent, spaces or separators)"
    )]
    Malformed,
    /// More than six digits after the point: a price is read in millionths.
    #[error("more than six decimals: a price is read in millionths of the currency unit")]
    TooManyDecimals,
    /// Above 1 000 000 currency units.
    #[error("above 1000000 currency units")]
    TooHigh,
}

/// The price `text` in millionths of the currency unit: `"0.40"` is `400_000`.
///
/// # Errors
///
/// [`PriceError`] for anything that is not digits with an optional point and one to six decimals,
/// and for a price above `1000000`. Zero is a price (a model that costs nothing says so).
pub fn parse_price(text: &str) -> Result<u64, PriceError> {
    if text.len() > MAX_PRICE_BYTES {
        return Err(PriceError::Malformed);
    }
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || fraction.is_some_and(|f| !digits(f)) {
        return Err(PriceError::Malformed);
    }
    let fraction = fraction.unwrap_or("");
    if fraction.len() > 6 {
        return Err(PriceError::TooManyDecimals);
    }
    // the integer part is checked against the bound as it is read, so a long run of digits cannot
    // overflow anything
    let mut units: u64 = 0;
    for b in whole.bytes() {
        units = units
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .filter(|n| *n <= MAX_PRICE_MICROS / 1_000_000)
            .ok_or(PriceError::TooHigh)?;
    }
    // the fraction, padded on the right to six digits
    let mut padded = fraction.bytes().chain(std::iter::repeat(b'0'));
    let mut millionths: u64 = 0;
    for _ in 0..6 {
        let digit = padded.next().map_or(0, |b| u64::from(b - b'0'));
        millionths = millionths * 10 + digit;
    }
    let micros = units * 1_000_000 + millionths;
    if micros > MAX_PRICE_MICROS {
        return Err(PriceError::TooHigh);
    }
    Ok(micros)
}

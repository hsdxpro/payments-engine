//! Exact fixed-point monetary amounts.

use std::fmt;
use std::str::FromStr;

/// Decimal places on every amount. The input allows four, the output prints
/// four.
const SCALE: u32 = 4;

/// Internal units in one whole currency unit.
const UNITS_PER_WHOLE: i64 = 10_i64.pow(SCALE);

/// An amount, as an exact count of ten-thousandths.
///
/// Integer rather than floating point so values round-trip exactly. Every
/// operation is checked, so debug and release behave the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money(i64);

impl Money {
    /// A zero amount.
    pub const ZERO: Self = Self(0);

    /// Builds an amount from a raw count of ten-thousandths.
    #[must_use]
    pub const fn from_units(units: i64) -> Self {
        Self(units)
    }

    /// Returns the amount as a raw count of ten-thousandths.
    #[must_use]
    pub const fn units(self) -> i64 {
        self.0
    }

    /// Adds two amounts, returning [`None`] on overflow.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    /// Subtracts `rhs`, returning [`None`] on overflow.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }

    /// Adds in a wider type, which cannot overflow.
    #[must_use]
    pub fn widening_add(self, rhs: Self) -> Total {
        Total(i128::from(self.0) + i128::from(rhs.0))
    }
}

/// The sum of two [`Money`] values.
///
/// Totals are derived from `available + held`, never stored, so they cannot
/// drift. The wider type makes the addition itself infallible. A result type
/// rather than a value type: it is compared and printed, never added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Total(i128);

/// Formats a raw unit count with exactly [`SCALE`] decimal places.
///
/// Callers pass `i128` so the fractional magnitude is always defined.
/// `i64::MIN.abs()` is not.
fn fmt_units(units: i128, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let divisor = i128::from(UNITS_PER_WHOLE);
    let whole = units / divisor;
    let frac = (units % divisor).unsigned_abs();
    // Division truncates towards zero, losing the sign between -1 and 0.
    if units < 0 && whole == 0 {
        f.write_str("-")?;
    }
    write!(f, "{whole}.{frac:0width$}", width = SCALE as usize)
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_units(i128::from(self.0), f)
    }
}

impl fmt::Display for Total {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_units(self.0, f)
    }
}

/// Returned when a field is not a decimal amount this engine can represent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseMoneyError;

/// Reads a run of ASCII digits, treating an empty run as zero. Rejects
/// anything else, including the signs and exponents `i64::from_str` allows.
fn digits(s: &str) -> Result<i64, ParseMoneyError> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseMoneyError);
    }
    if s.is_empty() {
        return Ok(0);
    }
    s.parse().map_err(|_| ParseMoneyError)
}

impl FromStr for Money {
    type Err = ParseMoneyError;

    /// Parses `digit* ('.' digit*)?`, needing at least one digit overall.
    ///
    /// The rule is that anything four decimal places can hold exactly is
    /// accepted, and anything else is refused rather than rounded. So `1.`,
    /// `.5` and `1.50000` all parse, since none of them is ambiguous or lossy,
    /// while `1.00005` does not. Signs and exponents are rejected outright.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
        // A lone "." has no digits at all and is not a number. Checked before
        // the trim below, which would otherwise leave ".0" looking the same.
        if whole.is_empty() && frac.is_empty() {
            return Err(ParseMoneyError);
        }
        // Trailing zeros carry no value, so they do not count against the four
        // places.
        let frac = frac.trim_end_matches('0');
        if frac.len() > SCALE as usize {
            return Err(ParseMoneyError);
        }

        // At most four digits, so the scaling cannot overflow.
        let frac = digits(frac)? * 10_i64.pow(SCALE - frac.len() as u32);
        digits(whole)?
            .checked_mul(UNITS_PER_WHOLE)
            .and_then(|units| units.checked_add(frac))
            .map(Self)
            .ok_or(ParseMoneyError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<i64, ParseMoneyError> {
        s.parse::<Money>().map(Money::units)
    }

    #[test]
    fn parses_accepted_shapes() {
        assert_eq!(parse("0"), Ok(0));
        assert_eq!(parse("1"), Ok(10_000));
        assert_eq!(parse("1.0"), Ok(10_000));
        assert_eq!(parse("1.5"), Ok(15_000));
        assert_eq!(parse("1.5000"), Ok(15_000));
        assert_eq!(parse("0.0001"), Ok(1));
        assert_eq!(parse("10.0001"), Ok(100_001));
        assert_eq!(parse("000001.05"), Ok(10_500));
        assert_eq!(parse("0.0010"), Ok(10));
        // Unambiguous and lossless, so accepted on the same rule.
        assert_eq!(parse("1."), Ok(10_000));
        assert_eq!(parse(".5"), Ok(5_000));
        assert_eq!(parse("."), Err(ParseMoneyError));
        // Zero stays zero however it is written.
        assert_eq!(parse(".0"), Ok(0));
        assert_eq!(parse(".0000"), Ok(0));
        assert_eq!(parse("0."), Ok(0));
    }

    #[test]
    fn accepts_trailing_zeros_past_four_places() {
        // Value-preserving, and a float formatter upstream may produce them.
        assert_eq!(parse("1.50000"), Ok(15_000));
        assert_eq!(parse("1.000000000000"), Ok(10_000));
        assert_eq!(parse("0.00010000"), Ok(1));
    }

    #[test]
    fn rejects_everything_else() {
        for input in [
            "",
            " ",
            ".",
            "-1",
            "-0.0000",
            "+1",
            "1e5",
            "1.00005",
            "abc",
            "1 000",
            "1.2.3",
            "922337203685478",
        ] {
            assert!(parse(input).is_err(), "{input:?} should not parse");
        }
    }

    #[test]
    fn displays_with_four_decimal_places() {
        assert_eq!(Money::ZERO.to_string(), "0.0000");
        assert_eq!(Money::from_units(15_000).to_string(), "1.5000");
        assert_eq!(Money::from_units(1).to_string(), "0.0001");
        assert_eq!(Money::from_units(-15_000).to_string(), "-1.5000");
    }

    #[test]
    fn keeps_the_sign_of_small_negative_values() {
        // The whole part is 0 here, so the sign has to be recovered by hand.
        assert_eq!(Money::from_units(-500).to_string(), "-0.0500");
        assert_eq!(Money::from_units(-1).to_string(), "-0.0001");
    }

    #[test]
    fn displays_the_extremes_without_panicking() {
        assert_eq!(
            Money::from_units(i64::MIN).to_string(),
            "-922337203685477.5808"
        );
        assert_eq!(
            Money::from_units(i64::MAX).to_string(),
            "922337203685477.5807"
        );
    }

    #[test]
    fn round_trips_through_display_and_parse() {
        for units in [0, 1, 15_000, 100_001, i64::MAX] {
            let printed = Money::from_units(units).to_string();
            assert_eq!(
                printed.parse::<Money>().unwrap().units(),
                units,
                "{printed}"
            );
        }
    }

    #[test]
    fn arithmetic_is_checked() {
        let max = Money::from_units(i64::MAX);
        assert_eq!(max.checked_add(Money::from_units(1)), None);
        assert_eq!(
            Money::from_units(i64::MIN).checked_sub(Money::from_units(1)),
            None
        );
        assert_eq!(max.checked_add(Money::ZERO), Some(max));
    }

    #[test]
    fn totals_cannot_overflow() {
        let max = Money::from_units(i64::MAX);
        assert_eq!(max.widening_add(max).to_string(), "1844674407370955.1614");
    }
}

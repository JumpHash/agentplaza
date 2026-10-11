//! Numbers and dimensions (docs/jhrl.md section 5).
//!
//! A number is exact (a rational over i128) until an operation forces it to be a real
//! (`ln`, a fractional power, `inf`). Unit literals convert to base units exactly, so
//! `1000 cr/d * 1 d == 1000 cr` holds without rounding. Overflow is an error, never a wrap.

use std::cmp::Ordering;
use std::fmt;

/// Exact rational or a float. Rationals are kept normalised (gcd 1, positive denominator).
#[derive(Clone, Copy, Debug)]
pub enum Num {
    Rat(i128, i128),
    Real(f64),
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

pub type NumResult = Result<Num, String>;

const OVERFLOW: &str = "integer overflow";

// Checked arithmetic returns `Result` (overflow and division by zero are errors), so the
// std operator traits don't fit; the method names say what they compute.
#[allow(clippy::should_implement_trait)]
impl Num {
    pub fn int(i: i128) -> Num {
        Num::Rat(i, 1)
    }

    pub fn rat(n: i128, d: i128) -> NumResult {
        if d == 0 {
            return Err("division by zero".into());
        }
        let g = gcd(n, d).max(1);
        let (mut n, mut d) = (n / g, d / g);
        if d < 0 {
            n = n.checked_neg().ok_or(OVERFLOW)?;
            d = d.checked_neg().ok_or(OVERFLOW)?;
        }
        Ok(Num::Rat(n, d))
    }

    /// A float, turned back into an exact integer when it is one (keeps state encodings canonical).
    pub fn real(x: f64) -> Num {
        if x.is_finite() && x.fract() == 0.0 && x.abs() < 9.0e15 {
            Num::Rat(x as i128, 1)
        } else {
            Num::Real(x)
        }
    }

    /// Parses a decimal literal such as `1_000`, `0.15` or `1e-3` exactly.
    pub fn parse_decimal(text: &str) -> Result<Num, String> {
        let t: String = text.chars().filter(|c| *c != '_').collect();
        let (mant, exp) = match t.find(['e', 'E']) {
            Some(i) => (&t[..i], t[i + 1..].parse::<i32>().map_err(|_| format!("bad exponent in `{text}`"))?),
            None => (&t[..], 0),
        };
        let (int_part, frac_part) = match mant.find('.') {
            Some(i) => (&mant[..i], &mant[i + 1..]),
            None => (mant, ""),
        };
        let digits = format!("{int_part}{frac_part}");
        let mut n: i128 = digits.parse().map_err(|_| format!("bad number `{text}`"))?;
        let mut d: i128 = 1;
        let mut e = exp - frac_part.len() as i32;
        while e > 0 {
            n = n.checked_mul(10).ok_or(OVERFLOW)?;
            e -= 1;
        }
        while e < 0 {
            d = d.checked_mul(10).ok_or(OVERFLOW)?;
            e += 1;
        }
        Num::rat(n, d)
    }

    pub fn to_f64(self) -> f64 {
        match self {
            Num::Rat(n, d) => n as f64 / d as f64,
            Num::Real(x) => x,
        }
    }

    pub fn is_zero(self) -> bool {
        match self {
            Num::Rat(n, _) => n == 0,
            Num::Real(x) => x == 0.0,
        }
    }

    pub fn as_int(self) -> Option<i128> {
        match self {
            Num::Rat(n, 1) => Some(n),
            _ => None,
        }
    }

    pub fn add(self, o: Num) -> NumResult {
        match (self, o) {
            (Num::Rat(a, b), Num::Rat(c, d)) => {
                let n = a.checked_mul(d).and_then(|x| c.checked_mul(b).and_then(|y| x.checked_add(y)));
                let den = b.checked_mul(d);
                match (n, den) {
                    (Some(n), Some(den)) => Num::rat(n, den),
                    _ => Err(OVERFLOW.into()),
                }
            }
            _ => Ok(Num::real(self.to_f64() + o.to_f64())),
        }
    }

    pub fn neg(self) -> NumResult {
        match self {
            Num::Rat(a, b) => Ok(Num::Rat(a.checked_neg().ok_or(OVERFLOW)?, b)),
            Num::Real(x) => Ok(Num::Real(-x)),
        }
    }

    pub fn sub(self, o: Num) -> NumResult {
        self.add(o.neg()?)
    }

    pub fn mul(self, o: Num) -> NumResult {
        match (self, o) {
            (Num::Rat(a, b), Num::Rat(c, d)) => {
                // Cross-reduce first to keep intermediate values small.
                let g1 = gcd(a, d).max(1);
                let g2 = gcd(c, b).max(1);
                let n = (a / g1).checked_mul(c / g2).ok_or(OVERFLOW)?;
                let den = (b / g2).checked_mul(d / g1).ok_or(OVERFLOW)?;
                Num::rat(n, den)
            }
            _ => Ok(Num::real(self.to_f64() * o.to_f64())),
        }
    }

    pub fn div(self, o: Num) -> NumResult {
        if o.is_zero() {
            return Err("division by zero".into());
        }
        match o {
            Num::Rat(c, d) => self.mul(Num::rat(d, c)?),
            Num::Real(x) => Ok(Num::real(self.to_f64() / x)),
        }
    }

    /// `self ^ e`. Exact for an integer exponent on a rational base.
    pub fn pow(self, e: Num) -> NumResult {
        if let (Num::Rat(..), Some(k)) = (self, e.as_int()) {
            if k.abs() > 4096 {
                return Ok(Num::real(self.to_f64().powf(k as f64)));
            }
            let mut acc = Num::int(1);
            for _ in 0..k.abs() {
                acc = acc.mul(self)?;
            }
            return if k < 0 { Num::int(1).div(acc) } else { Ok(acc) };
        }
        let r = self.to_f64().powf(e.to_f64());
        if r.is_nan() {
            return Err(format!("{self} ^ {e} is not a real number"));
        }
        Ok(Num::real(r))
    }

    pub fn floor(self) -> NumResult {
        match self {
            Num::Rat(n, d) => Ok(Num::int(n.div_euclid(d))),
            Num::Real(x) => Ok(Num::real(x.floor())),
        }
    }

    pub fn ceil(self) -> NumResult {
        self.neg()?.floor()?.neg()
    }

    /// Round half away from zero.
    pub fn round(self) -> NumResult {
        match self {
            Num::Rat(n, d) => {
                let twice = n.checked_mul(2).ok_or(OVERFLOW)?;
                let q = if n >= 0 { (twice + d).div_euclid(2 * d) } else { -((-twice + d).div_euclid(2 * d)) };
                Ok(Num::int(q))
            }
            Num::Real(x) => Ok(Num::real(x.round())),
        }
    }

    pub fn abs(self) -> NumResult {
        if self.cmp_num(Num::int(0)) == Ordering::Less {
            self.neg()
        } else {
            Ok(self)
        }
    }

    /// Total order: rationals exactly, floats by value (NaN never arises; see `pow`).
    pub fn cmp_num(self, o: Num) -> Ordering {
        match (self, o) {
            (Num::Rat(a, b), Num::Rat(c, d)) => match (a.checked_mul(d), c.checked_mul(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => self.to_f64().total_cmp(&o.to_f64()),
            },
            _ => self.to_f64().partial_cmp(&o.to_f64()).unwrap_or(Ordering::Equal),
        }
    }
}

impl PartialEq for Num {
    fn eq(&self, o: &Num) -> bool {
        self.cmp_num(*o) == Ordering::Equal
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Num::Rat(n, 1) => write!(f, "{n}"),
            Num::Rat(n, d) => {
                // Terminating decimals print as decimals, others as fractions.
                let mut dd = d;
                let mut k = 0;
                while dd % 10 == 0 {
                    dd /= 10;
                    k += 1;
                }
                while dd % 2 == 0 || dd % 5 == 0 {
                    if dd % 2 == 0 {
                        dd /= 2;
                    } else {
                        dd /= 5;
                    }
                    k += 1;
                }
                if dd == 1 && k <= 18 {
                    let scale = 10i128.pow(k);
                    let scaled = n * (scale / d);
                    let sign = if scaled < 0 { "-" } else { "" };
                    let a = scaled.abs();
                    let s = format!("{}.{:0width$}", a / scale, a % scale, width = k as usize);
                    write!(f, "{sign}{}", s.trim_end_matches('0'))
                } else {
                    write!(f, "{n}/{d}")
                }
            }
            Num::Real(x) => {
                if x.is_infinite() {
                    write!(f, "{}inf", if x < 0.0 { "-" } else { "" })
                } else {
                    let s = format!("{x:.6}");
                    write!(f, "{}", s.trim_end_matches('0').trim_end_matches('.'))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------- dimensions

/// Base dimensions, in this order.
pub const BASES: [&str; 5] = ["credits", "money", "time", "information", "tokens"];
const BASE_UNIT: [&str; 5] = ["cr", "USD", "s", "B", "tok"];

/// An exponent vector over the five base dimensions. Count is dimensionless.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Dim(pub [i8; 5]);

#[allow(clippy::should_implement_trait)]
impl Dim {
    pub const NONE: Dim = Dim([0; 5]);
    pub const CREDITS: Dim = Dim([1, 0, 0, 0, 0]);
    pub const MONEY: Dim = Dim([0, 1, 0, 0, 0]);
    pub const TIME: Dim = Dim([0, 0, 1, 0, 0]);
    pub const INFO: Dim = Dim([0, 0, 0, 1, 0]);
    pub const TOKENS: Dim = Dim([0, 0, 0, 0, 1]);

    pub fn is_none(self) -> bool {
        self == Dim::NONE
    }
    pub fn mul(self, o: Dim) -> Dim {
        let mut r = [0; 5];
        for (i, x) in r.iter_mut().enumerate() {
            *x = self.0[i] + o.0[i];
        }
        Dim(r)
    }
    pub fn inv(self) -> Dim {
        let mut r = self.0;
        for x in r.iter_mut() {
            *x = -*x;
        }
        Dim(r)
    }
    pub fn div(self, o: Dim) -> Dim {
        self.mul(o.inv())
    }
    pub fn pow(self, k: i128) -> Dim {
        let mut r = self.0;
        for x in r.iter_mut() {
            *x = (*x as i128 * k) as i8;
        }
        Dim(r)
    }

    /// The dimension a type name stands for (`Credits`, `Duration`, `Frequency` …).
    pub fn of_type_name(name: &str) -> Option<Dim> {
        Some(match name {
            "Number" | "Int" | "Count" => Dim::NONE,
            "Credits" => Dim::CREDITS,
            "Money" => Dim::MONEY,
            "Duration" => Dim::TIME,
            "Information" => Dim::INFO,
            "Tokens" => Dim::TOKENS,
            "Frequency" => Dim::TIME.inv(),
            _ => return None,
        })
    }

    /// A readable name: the type name when there is one, else the base-unit expression.
    pub fn describe(self) -> String {
        for n in ["Number", "Credits", "Money", "Duration", "Information", "Tokens", "Frequency"] {
            if Dim::of_type_name(n) == Some(self) {
                return n.to_string();
            }
        }
        format!("[{}]", self.unit_suffix().trim())
    }

    /// Base-unit suffix used when printing a quantity, e.g. ` cr`, ` cr/s`, ` /s`.
    pub fn unit_suffix(self) -> String {
        if self.is_none() {
            return String::new();
        }
        let mut num = Vec::new();
        let mut den = Vec::new();
        for (i, &e) in self.0.iter().enumerate() {
            let u = BASE_UNIT[i];
            let s = |k: i8| if k.abs() == 1 { u.to_string() } else { format!("{u}^{}", k.abs()) };
            match e.cmp(&0) {
                Ordering::Greater => num.push(s(e)),
                Ordering::Less => den.push(s(e)),
                Ordering::Equal => {}
            }
        }
        let mut out = format!(" {}", num.join("·"));
        for d in den {
            out.push('/');
            out.push_str(&d);
        }
        out
    }
}

/// A unit name and what one of it is in base units.
pub fn unit(name: &str) -> Option<(Num, Dim)> {
    let r = |n: i128, d: i128| Num::Rat(n, d);
    Some(match name {
        "cr" => (r(1, 1), Dim::CREDITS),
        "kcr" => (r(1_000, 1), Dim::CREDITS),
        "Mcr" => (r(1_000_000, 1), Dim::CREDITS),
        "USD" => (r(1, 1), Dim::MONEY),
        "mUSD" => (r(1, 1_000), Dim::MONEY),
        "ms" => (r(1, 1_000), Dim::TIME),
        "s" => (r(1, 1), Dim::TIME),
        "min" => (r(60, 1), Dim::TIME),
        "h" => (r(3_600, 1), Dim::TIME),
        "d" => (r(86_400, 1), Dim::TIME),
        "wk" => (r(604_800, 1), Dim::TIME),
        "bit" => (r(1, 8), Dim::INFO),
        "B" => (r(1, 1), Dim::INFO),
        "KiB" => (r(1 << 10, 1), Dim::INFO),
        "MiB" => (r(1 << 20, 1), Dim::INFO),
        "GiB" => (r(1 << 30, 1), Dim::INFO),
        "TiB" => (r(1 << 40, 1), Dim::INFO),
        "kB" => (r(1_000, 1), Dim::INFO),
        "MB" => (r(1_000_000, 1), Dim::INFO),
        "GB" => (r(1_000_000_000, 1), Dim::INFO),
        "TB" => (r(1_000_000_000_000, 1), Dim::INFO),
        "tok" => (r(1, 1), Dim::TOKENS),
        "ktok" => (r(1_000, 1), Dim::TOKENS),
        "Mtok" => (r(1_000_000, 1), Dim::TOKENS),
        "%" => (r(1, 100), Dim::NONE),
        "ppm" => (r(1, 1_000_000), Dim::NONE),
        _ => return None,
    })
}

/// Evaluates a unit expression such as `cr`, `cr/d`, `/min` or `cr/Mtok` to (factor, dimension).
pub fn unit_expr(text: &str) -> Result<(Num, Dim), String> {
    let mut factor = Num::int(1);
    let mut dim = Dim::NONE;
    for (i, part) in text.split('/').enumerate() {
        if part.is_empty() && i == 0 {
            continue; // leading `/` as in `/min`
        }
        let (f, d) = unit(part).ok_or_else(|| format!("unknown unit `{part}`"))?;
        if i == 0 {
            factor = factor.mul(f)?;
            dim = dim.mul(d);
        } else {
            factor = factor.div(f)?;
            dim = dim.div(d);
        }
    }
    Ok((factor, dim))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact() {
        assert_eq!(Num::parse_decimal("0.15").unwrap(), Num::rat(3, 20).unwrap());
        assert_eq!(Num::parse_decimal("1_000").unwrap(), Num::int(1000));
        assert_eq!(Num::parse_decimal("1e-3").unwrap(), Num::rat(1, 1000).unwrap());
        assert_eq!(format!("{}", Num::parse_decimal("0.44").unwrap()), "0.44");
        assert_eq!(format!("{}", Num::rat(1, 3).unwrap()), "1/3");
    }

    #[test]
    fn rates_round_trip_exactly() {
        let (f, d) = unit_expr("cr/d").unwrap();
        let (g, e) = unit_expr("d").unwrap();
        assert_eq!(f.mul(g).unwrap(), Num::int(1));
        assert_eq!(d.mul(e), Dim::CREDITS);
        assert_eq!(unit_expr("/min").unwrap().1, Dim::TIME.inv());
        assert_eq!(unit_expr("KiB").unwrap().0, Num::int(16384).div(Num::int(16)).unwrap());
        assert!(unit_expr("parsec").is_err());
    }

    #[test]
    fn rounding_and_powers() {
        assert_eq!(Num::rat(5, 2).unwrap().round().unwrap(), Num::int(3));
        assert_eq!(Num::rat(-5, 2).unwrap().round().unwrap(), Num::int(-3));
        assert_eq!(Num::int(2).pow(Num::int(-2)).unwrap(), Num::rat(1, 4).unwrap());
        assert_eq!(Num::rat(-7, 2).unwrap().floor().unwrap(), Num::int(-4));
        assert!(Num::int(i128::MAX).add(Num::int(1)).is_err());
    }
}

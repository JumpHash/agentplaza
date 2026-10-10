//! The harness's configuration, read from the guest kernel command line.
//!
//! The guest's `init` is started by the kernel with the command line the VMM
//! built, so `/proc/cmdline` is the channel the orchestrator uses to tell the
//! guest who it is. The flags this side reads (the frozen slice contract):
//!
//! ```text
//! plaza.cid=<n>    this VM's vsock CID (required)
//! plaza.port=<n>   the orchestrator's vsock port (optional, default 5000)
//! plaza.vm=<n>     this VM's index within the spawn (optional, default 0)
//! ```
//!
//! `plaza.harness=1` is consumed by the guest's `init`, not by the harness.

use std::fmt;

/// The orchestrator's vsock port when the command line does not name one.
pub const DEFAULT_PORT: u32 = 5000;

/// The smallest CID a guest may be given: 0 is `VMADDR_CID_ANY`, 1 is
/// `VMADDR_CID_LOCAL` and 2 is `VMADDR_CID_HOST`, all reserved by the vsock
/// stack, so a caller assigns a guest CID of 3 or more.
pub const MIN_GUEST_CID: u32 = 3;

/// Everything the harness needs to open its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// This VM's vsock CID, reported in `hello`.
    pub cid: u32,
    /// The orchestrator's vsock port.
    pub port: u32,
    /// This VM's index within the spawn, reported in `hello`.
    pub vm: u32,
}

/// Why the command line could not be turned into a [`Config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The command line has no `plaza.cid=<n>` flag.
    MissingCid,
    /// A flag's value is not a number: `{flag}={value}`
    BadValue {
        /// The flag as it appeared, without its value.
        flag: String,
        /// The value that did not parse.
        value: String,
    },
    /// CID {0} is reserved: a guest CID is [`MIN_GUEST_CID`] or more
    ReservedCid(u32),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCid => write!(f, "the kernel command line has no plaza.cid=<n> flag"),
            Self::BadValue { flag, value } => {
                write!(f, "{flag}={value} is not a number")
            }
            Self::ReservedCid(cid) => write!(
                f,
                "plaza.cid={cid} is reserved (0 is VMADDR_CID_ANY, 1 VMADDR_CID_LOCAL and 2 VMADDR_CID_HOST); a guest CID is {MIN_GUEST_CID} or more"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parses a kernel command line (`/proc/cmdline`).
    ///
    /// Flags are whitespace-separated `key=value` tokens; a token that is not
    /// one of the three above is ignored, as is a token that repeats (the
    /// first occurrence wins, which keeps the result independent of how the
    /// builder ordered the tail of the command line).
    pub fn parse(cmdline: &str) -> Result<Self, ConfigError> {
        let value_of = |flag: &str| -> Option<&str> {
            cmdline
                .split_whitespace()
                .find_map(|token| token.strip_prefix(flag))
        };
        let number = |flag: &str, value: &str| -> Result<u32, ConfigError> {
            value.parse().map_err(|_| ConfigError::BadValue {
                flag: flag.to_string(),
                value: value.to_string(),
            })
        };

        let cid = value_of("plaza.cid=").ok_or(ConfigError::MissingCid)?;
        // The CID is the only flag that is not optional, so it is parsed
        // first; a bad CID is reported even if the other flags are missing.
        let cid = number("plaza.cid", cid)?;
        if cid < MIN_GUEST_CID {
            return Err(ConfigError::ReservedCid(cid));
        }
        let port = match value_of("plaza.port=") {
            Some(value) => number("plaza.port", value)?,
            None => DEFAULT_PORT,
        };
        let vm = match value_of("plaza.vm=") {
            Some(value) => number("plaza.vm", value)?,
            None => 0,
        };
        Ok(Self { cid, port, vm })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_command_line_parses() {
        let config = Config::parse("console=ttyS0 plaza.harness=1 plaza.cid=3")
            .expect("the slice's command line parses");
        assert_eq!(
            config,
            Config {
                cid: 3,
                port: DEFAULT_PORT,
                vm: 0
            }
        );
    }

    #[test]
    fn optional_flags_override_their_defaults() {
        let config = Config::parse("plaza.cid=7 plaza.port=6000 plaza.vm=4")
            .expect("explicit flags parse");
        assert_eq!(
            config,
            Config {
                cid: 7,
                port: 6000,
                vm: 4
            }
        );
    }

    #[test]
    fn a_missing_or_bad_cid_is_reported() {
        assert_eq!(Config::parse("console=ttyS0 plaza.port=5000"), Err(ConfigError::MissingCid));
        assert_eq!(
            Config::parse("plaza.cid=three"),
            Err(ConfigError::BadValue {
                flag: "plaza.cid".to_string(),
                value: "three".to_string()
            })
        );
    }

    #[test]
    fn a_reserved_cid_is_refused() {
        // 0/1/2 belong to the vsock stack, not to a guest.
        for reserved in [0, 1, 2] {
            assert_eq!(
                Config::parse(&format!("plaza.cid={reserved}")),
                Err(ConfigError::ReservedCid(reserved)),
                "cid {reserved}"
            );
        }
        assert!(Config::parse("plaza.cid=3").is_ok());
    }

    #[test]
    fn a_bad_optional_flag_is_reported_not_ignored() {
        assert_eq!(
            Config::parse("plaza.cid=3 plaza.port=x"),
            Err(ConfigError::BadValue {
                flag: "plaza.port".to_string(),
                value: "x".to_string()
            })
        );
    }

    #[test]
    fn lookalike_and_repeated_tokens_do_not_confuse_the_parser() {
        // `plaza.cid=` must not match inside another token, and the first
        // occurrence of a repeated flag wins.
        let config = Config::parse("foo.plaza.cid=9 plaza.cid=3 plaza.cid=4").expect("parses");
        assert_eq!(config.cid, 3);
    }
}

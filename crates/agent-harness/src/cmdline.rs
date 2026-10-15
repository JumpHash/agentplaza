//! The harness's configuration, read from the guest kernel command line.
//!
//! The guest's `init` is started by the kernel with the command line the VMM
//! built, so `/proc/cmdline` is the channel the orchestrator uses to tell the
//! guest how to reach it. The flags this side reads:
//!
//! ```text
//! plaza.port=<n>       the orchestrator's vsock port (optional, default 5000)
//! plaza.vm=<n>         this VM's index within the spawn (optional, default 0)
//! plaza.role=<role>    the agent's role (optional, default none)
//! plaza.keepalive=1    run the long-lived keepalive session (optional, default off)
//! plaza.template=1    a warm template: announce readiness, then retry the hub every 25 ms
//!                     until its first welcome (optional, default off)
//! ```
//!
//! `plaza.harness=1` is consumed by the guest's `init`, not by the harness;
//! so is `plaza.keepalive=1`, which [`Config::keepalive`] also reads — an
//! agent whose command line says keepalive implies the harness, and the flag
//! is what tells the harness to run the v2 loop instead of the frozen v1
//! session.
//!
//! The guest's **CID is deliberately not here**. One command line is shared by
//! every VM of a spawn, while every VM is given its own CID by the device; a
//! CID taken from the command line would therefore be wrong for all but one
//! VM. The harness asks the vsock device for the CID it was actually given
//! (see [`crate::vsock::device_guest_cid`]), so `hello` names the CID the
//! packets carry rather than a guess that disagrees with them.

use std::fmt;

/// The orchestrator's vsock port when the command line does not name one.
pub const DEFAULT_PORT: u32 = 5000;

/// Everything the harness needs to open its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The orchestrator's vsock port.
    pub port: u32,
    /// This VM's index within the spawn, reported in `hello`.
    pub vm: u32,
    /// The agent's role, from `plaza.role=`, or `None` when the guest's command
    /// line names none. It is offered in `hello`; the hub's `welcome` decides
    /// the identity the agent carries for its lifetime.
    pub role: Option<String>,
    /// Whether this guest runs the long-lived keepalive session
    /// (`plaza.keepalive=1`) instead of the frozen one-shot session.
    pub keepalive: bool,
    /// Whether this guest is a warm template (`plaza.template=1`): one the host snapshots while
    /// it waits for its first welcome, so it announces readiness and retries the hub quickly.
    pub template: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            vm: 0,
            role: None,
            keepalive: false,
            template: false,
        }
    }
}

/// Why the command line could not be turned into a [`Config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A flag's value is not a number: `{flag}={value}`
    BadValue {
        /// The flag as it appeared, without its value.
        flag: String,
        /// The value that did not parse.
        value: String,
    },
    /// `plaza.role=` is not a usable role name: {reason}
    BadRole {
        /// Why the value cannot be a role name.
        reason: &'static str,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadValue { flag, value } => write!(f, "{flag}={value} is not a number"),
            Self::BadRole { reason } => write!(f, "plaza.role= is not a role name: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parses a kernel command line (`/proc/cmdline`).
    ///
    /// Flags are whitespace-separated `key=value` tokens; a token that is not
    /// one of the four above is ignored, as is a token that repeats (the first
    /// occurrence wins, which keeps the result independent of how the builder
    /// ordered the tail of the command line). A command line with no flag is
    /// the defaults, so the harness can be started by hand.
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

        let port = match value_of("plaza.port=") {
            Some(value) => number("plaza.port", value)?,
            None => DEFAULT_PORT,
        };
        let vm = match value_of("plaza.vm=") {
            Some(value) => number("plaza.vm", value)?,
            None => 0,
        };
        // `plaza.keepalive=0` is off and any other number is on: the guest's
        // `init` reads this flag the same way (`parse_flag`), so the two sides
        // of one guest never disagree about whether it is a long-lived agent.
        let keepalive = match value_of("plaza.keepalive=") {
            Some(value) => number("plaza.keepalive", value)? != 0,
            None => false,
        };
        let template = match value_of("plaza.template=") {
            Some(value) => number("plaza.template", value)? != 0,
            None => false,
        };
        let role = match value_of("plaza.role=") {
            // An empty value is "no role", not a role named "".
            Some("") => None,
            Some(value) => Some(validate_role(value)?),
            None => None,
        };
        Ok(Self {
            port,
            vm,
            role,
            keepalive,
            template,
        })
    }
}

/// Checks that `value` can be a role name, and returns it.
///
/// A role is an identity: it goes into `hello` (where JSON would escape it)
/// and into the operator-visible transcript, so a value carrying control
/// characters could forge transcript lines, or present two different
/// identities that print the same. `plaza.role=` cannot contain whitespace
/// anyway — the command line is whitespace-separated — so this only has to
/// refuse the invisible.
fn validate_role(value: &str) -> Result<String, ConfigError> {
    if value.chars().any(char::is_control) {
        return Err(ConfigError::BadRole {
            reason: "it contains a control character",
        });
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_v1_and_fleet_command_lines_parse() {
        assert_eq!(
            Config::parse("console=ttyS0 plaza.harness=1").unwrap(),
            Config::default()
        );
        assert_eq!(Config::default().port, DEFAULT_PORT);
        let fleet = Config::parse(
            "console=ttyS0 plaza.role=builder plaza.keepalive=1 plaza.vm=4 plaza.port=6000",
        )
        .unwrap();
        assert_eq!(
            fleet,
            Config {
                port: 6000,
                vm: 4,
                role: Some("builder".to_string()),
                keepalive: true,
                template: false,
            }
        );
        assert!(!Config::parse("plaza.keepalive=0").unwrap().keepalive);
        assert!(Config::parse("plaza.template=1").unwrap().template);
        assert!(!Config::parse("plaza.template=0").unwrap().template);
        assert!(!Config::default().template);
        assert_eq!(Config::parse("plaza.role=").unwrap().role, None);
    }

    #[test]
    fn lookalike_and_repeated_tokens_do_not_confuse_the_parser() {
        assert_eq!(
            Config::parse("foo.plaza.vm=9 plaza.vm=3 plaza.vm=4")
                .unwrap()
                .vm,
            3
        );
    }

    #[test]
    fn bad_values_and_control_characters_are_refused() {
        assert_eq!(
            Config::parse("plaza.keepalive=yes"),
            Err(ConfigError::BadValue {
                flag: "plaza.keepalive".to_string(),
                value: "yes".to_string()
            })
        );
        assert!(Config::parse("plaza.port=x").is_err());
        assert!(Config::parse("plaza.template=yes").is_err());
        assert_eq!(
            Config::parse("plaza.role=ok\u{1b}[2K"),
            Err(ConfigError::BadRole {
                reason: "it contains a control character"
            })
        );
    }
}

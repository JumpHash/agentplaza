//! The command line: `join`, `run`, `status`, `leave`, plus `--help` and `--version`.
//!
//! ```text
//! apz-workstation join --hub URL --code CODE [--name N] [--account agent:<user>|owner] [--pin PIN] [--state-dir DIR]
//! apz-workstation run [--state-dir DIR]
//! apz-workstation status [--state-dir DIR]
//! apz-workstation leave [--state-dir DIR]
//! ```

use std::path::PathBuf;

/// The usage text.
pub const USAGE: &str = "\
usage: apz-workstation join --hub URL --code CODE [--name N] [--account agent:<user>|owner] [--pin PIN] [--state-dir DIR]
       apz-workstation join --from-node vsock://2:5101 [--name N] [--account agent:<user>|owner] [--state-dir DIR]
       apz-workstation run [--state-dir DIR] [--fake-desktop]
       apz-workstation status [--state-dir DIR]
       apz-workstation leave [--state-dir DIR]";

/// One parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Join(JoinArgs),
    /// `fake_desktop`: serve the in-memory desktop (`--fake-desktop`), for the daemon's own tests.
    Run {
        state: Option<PathBuf>,
        fake_desktop: bool,
    },
    Status {
        state: Option<PathBuf>,
    },
    Leave {
        state: Option<PathBuf>,
    },
    Help,
    Version,
}

/// The arguments of `join`: either a hub and its code, or `--from-node`, the `vsock://` address of the node's config
/// port, which serves the hub, the pin and the code (then `hub` and `code` are empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinArgs {
    pub hub: String,
    pub code: String,
    pub from_node: Option<String>,
    pub name: Option<String>,
    pub account: Option<AccountArg>,
    pub pin: Option<String>,
    pub state: Option<PathBuf>,
}

/// `--account`: the separate agent account (with its name), or the owner's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountArg {
    Agent(String),
    Owner,
}

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        return Ok(Command::Version);
    }
    let mut words = args.iter();
    let Some(command) = words.next() else {
        return Ok(Command::Help);
    };
    // `--fake-desktop` is a bare flag, so it is taken out before the name-value flags are read.
    let fake_desktop = words.clone().any(|w| w.as_str() == "--fake-desktop");
    if fake_desktop && command.as_str() != "run" {
        return Err("--fake-desktop is an option of run".to_string());
    }
    let rest: Vec<String> = words
        .filter(|w| w.as_str() != "--fake-desktop")
        .cloned()
        .collect();
    let flags = Flags::collect(&rest)?;
    match command.as_str() {
        "join" => {
            flags.only(&[
                "--hub",
                "--code",
                "--from-node",
                "--name",
                "--account",
                "--pin",
                "--state-dir",
            ])?;
            let name = flags.get("--name").map(str::to_string);
            if name
                .as_deref()
                .is_some_and(|n| n.trim().is_empty() || n.chars().count() > 64)
            {
                return Err("--name is 1 to 64 characters".to_string());
            }
            let account = flags.get("--account").map(parse_account).transpose()?;
            if let Some(node) = flags.get("--from-node") {
                if flags.get("--hub").is_some()
                    || flags.get("--code").is_some()
                    || flags.get("--pin").is_some()
                {
                    return Err("--from-node takes the hub, its pin and the code from the node: give it alone".to_string());
                }
                if !node.starts_with("vsock://") {
                    return Err(
                        "--from-node is a vsock:// address of the node's config port".to_string(),
                    );
                }
                return Ok(Command::Join(JoinArgs {
                    hub: String::new(),
                    code: String::new(),
                    from_node: Some(node.to_string()),
                    name,
                    account,
                    pin: None,
                    state: flags.state(),
                }));
            }
            let hub = flags.require("--hub")?;
            if !(hub.starts_with("https://")
                || hub.starts_with("http://")
                || hub.starts_with("vsock://"))
            {
                return Err(
                    "--hub is an https:// URL, or a vsock:// address through the node".to_string(),
                );
            }
            let code = flags.require("--code")?;
            if code.is_empty() || code.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err("--code is one word with no spaces".to_string());
            }
            let pin = flags.get("--pin").map(str::to_string);
            Ok(Command::Join(JoinArgs {
                hub: hub.trim_end_matches('/').to_string(),
                code: code.to_string(),
                from_node: None,
                name,
                account,
                pin,
                state: flags.state(),
            }))
        }
        "run" => {
            flags.only(&["--state-dir"])?;
            Ok(Command::Run {
                state: flags.state(),
                fake_desktop,
            })
        }
        "status" => {
            flags.only(&["--state-dir"])?;
            Ok(Command::Status {
                state: flags.state(),
            })
        }
        "leave" => {
            flags.only(&["--state-dir"])?;
            Ok(Command::Leave {
                state: flags.state(),
            })
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

/// `agent:<user>` or `owner`.
pub fn parse_account(text: &str) -> Result<AccountArg, String> {
    if text == "owner" {
        return Ok(AccountArg::Owner);
    }
    match text.strip_prefix("agent:") {
        Some(user) if !user.is_empty() && !user.chars().any(|c| c.is_whitespace() || c == '/') => {
            Ok(AccountArg::Agent(user.to_string()))
        }
        _ => Err("--account is agent:<user> or owner".to_string()),
    }
}

/// The `--flag value` and `--flag=value` pairs of a command.
struct Flags {
    pairs: Vec<(String, String)>,
}

impl Flags {
    fn collect(words: &[String]) -> Result<Self, String> {
        let mut pairs = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let word = &words[i];
            if let Some((name, value)) = word
                .split_once('=')
                .filter(|(name, _)| name.starts_with("--"))
            {
                pairs.push((name.to_string(), value.to_string()));
                i += 1;
                continue;
            }
            if !word.starts_with("--") {
                return Err(format!("unexpected argument {word:?}"));
            }
            let value = words
                .get(i + 1)
                .ok_or_else(|| format!("{word} needs a value"))?;
            pairs.push((word.clone(), value.clone()));
            i += 2;
        }
        let mut seen: Vec<&str> = Vec::new();
        for (name, _) in &pairs {
            if seen.contains(&name.as_str()) {
                return Err(format!("{name} is given twice"));
            }
            seen.push(name);
        }
        Ok(Flags { pairs })
    }

    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self
            .pairs
            .iter()
            .find(|(name, _)| !allowed.contains(&name.as_str()))
        {
            Some((name, _)) => Err(format!("{name} is not an option of this command")),
            None => Ok(()),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn require(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("{name} is required"))
    }

    fn state(&self) -> Option<PathBuf> {
        self.get("--state-dir").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn join_takes_its_options_and_refuses_a_missing_or_bad_hub_code_and_name() {
        // join_takes_the_hub_code_and_options: the hub, code, name, account and state dir are read.
        {
            let parsed = parse(&words(
                "join --hub https://hub.example:7443/ --code ABC-123 --name studio --account agent:agentws --state-dir /tmp/s",
            ))
            .unwrap();
            assert_eq!(
                parsed,
                Command::Join(JoinArgs {
                    hub: "https://hub.example:7443".into(),
                    code: "ABC-123".into(),
                    from_node: None,
                    name: Some("studio".into()),
                    account: Some(AccountArg::Agent("agentws".into())),
                    pin: None,
                    state: Some(PathBuf::from("/tmp/s")),
                })
            );
        }
        // equals_signs_work_and_owner_is_an_account: the --opt=value form works and owner is an account.
        {
            let parsed = parse(&words(
                "join --hub=http://127.0.0.1:7443 --code=X1 --account=owner",
            ))
            .unwrap();
            match parsed {
                Command::Join(args) => {
                    assert_eq!(args.hub, "http://127.0.0.1:7443");
                    assert_eq!(args.account, Some(AccountArg::Owner));
                }
                other => panic!("not a join: {other:?}"),
            }
        }
        // join_takes_a_vsock_hub_with_its_pin_or_the_node_alone: a vsock hub takes a pin, and a node
        // alone takes no code or hub.
        {
            let parsed = parse(&words(
                "join --hub vsock://2:5100 --code ABC --pin cGlu --state-dir /tmp/s",
            ))
            .unwrap();
            match parsed {
                Command::Join(args) => {
                    assert_eq!(args.hub, "vsock://2:5100");
                    assert_eq!(args.pin.as_deref(), Some("cGlu"));
                    assert_eq!(args.from_node, None);
                }
                other => panic!("not a join: {other:?}"),
            }

            let parsed = parse(&words("join --from-node vsock://2:5101 --name studio")).unwrap();
            match parsed {
                Command::Join(args) => {
                    assert_eq!(args.from_node.as_deref(), Some("vsock://2:5101"));
                    assert!(args.hub.is_empty() && args.code.is_empty());
                    assert_eq!(args.name.as_deref(), Some("studio"));
                }
                other => panic!("not a join: {other:?}"),
            }
            // The node's config carries the code, so no code or hub may come with --from-node.
            assert!(parse(&words("join --from-node vsock://2:5101 --code X")).is_err());
            assert!(parse(&words("join --from-node vsock://2:5101 --hub https://h")).is_err());
            assert!(parse(&words("join --from-node https://h")).is_err());
            assert!(parse(&words("join --hub ftp://h --code X")).is_err());
        }
        // join_refuses_a_missing_or_bad_hub_code_and_name: a missing or bad hub, code or name is refused.
        {
            assert!(parse(&words("join --code X")).is_err());
            assert!(parse(&words("join --hub ftp://h --code X")).is_err());
            assert!(parse(&words("join --hub https://h --code 'two words'")).is_err());
            assert!(parse(&words("join --hub https://h --code X --name")).is_err());
            assert!(parse(&words("join --hub https://h --code X --name ' '")).is_err());
        }
    }

    #[test]
    fn account_must_be_an_agent_user_or_owner() {
        assert_eq!(
            parse_account("agent:bob"),
            Ok(AccountArg::Agent("bob".into()))
        );
        assert!(parse_account("agent:").is_err());
        assert!(parse_account("agent:a/b").is_err());
        assert!(parse_account("root").is_err());
    }

    #[test]
    fn run_status_and_leave_take_only_the_state_dir() {
        assert_eq!(
            parse(&words("run --state-dir /x")).unwrap(),
            Command::Run {
                state: Some(PathBuf::from("/x")),
                fake_desktop: false,
            }
        );
        assert_eq!(
            parse(&words("run --fake-desktop")).unwrap(),
            Command::Run {
                state: None,
                fake_desktop: true,
            }
        );
        assert!(parse(&words("status --fake-desktop")).is_err());
        assert_eq!(
            parse(&words("status")).unwrap(),
            Command::Status { state: None }
        );
        assert!(parse(&words("run --hub https://h")).is_err());
        assert!(parse(&words("leave extra")).is_err());
    }

    #[test]
    fn help_version_and_no_arguments_are_not_errors() {
        assert_eq!(parse(&[]).unwrap(), Command::Help);
        assert_eq!(parse(&words("--version")).unwrap(), Command::Version);
        assert_eq!(parse(&words("run -h")).unwrap(), Command::Help);
        assert!(parse(&words("explode")).is_err());
        assert!(parse(&words("run --state-dir a --state-dir b")).is_err());
    }
}

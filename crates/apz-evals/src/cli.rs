//! The suite's command line.
//!
//! ```text
//! apz-evals [--out FILE] [--compare BASELINE] [--tolerance PERCENT] [--model PROVIDER/MODEL]
//!           [--env-file FILE] [--bin-dir DIR] [--tasks DIR] [--only TASK] [--timeout SECONDS]
//! ```
//!
//! Without `--model` the suite runs its scripted models (`mock/script` for the builder, `mock/script-review` for the
//! reviewer). With one, both seats run that model; its keys go in the `--env-file` the hub reads.

use std::path::PathBuf;

/// What the suite was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Where the report is written (also printed).
    pub out: Option<PathBuf>,
    /// A baseline report to compare the run with; a regression makes the run fail.
    pub compare: Option<PathBuf>,
    /// How much a task's calls, turns, tokens or cost may grow over the baseline, as a fraction (0.10 = 10 %).
    pub tolerance: f64,
    /// `mock` (the scripted models) or `provider/model` for both seats.
    pub model: String,
    /// The env file the hub reads (provider keys for a real model); empty when none is named.
    pub env_file: Option<PathBuf>,
    /// The folder holding `apid`, `vsockd` and `agent-harness` (default: the folder this binary is in).
    pub bin_dir: PathBuf,
    /// The tasks' folder (default: `tasks` beside this crate).
    pub tasks: PathBuf,
    /// Run only this task.
    pub only: Option<String>,
    /// How long one task may take, in seconds.
    pub timeout_s: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            out: None,
            compare: None,
            tolerance: 0.10,
            model: "mock".into(),
            env_file: None,
            bin_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(".")),
            tasks: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tasks"),
            only: None,
            timeout_s: 300,
        }
    }
}

/// Reads the arguments after the program's name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => options.out = Some(PathBuf::from(value()?)),
            "--compare" => options.compare = Some(PathBuf::from(value()?)),
            "--tolerance" => {
                let percent: f64 = value()?
                    .parse()
                    .map_err(|err| format!("--tolerance: {err}"))?;
                if !(0.0..=100.0).contains(&percent) {
                    return Err(format!("--tolerance must be 0 to 100, not {percent}"));
                }
                options.tolerance = percent / 100.0;
            }
            "--model" => options.model = value()?,
            "--env-file" => options.env_file = Some(PathBuf::from(value()?)),
            "--bin-dir" => options.bin_dir = PathBuf::from(value()?),
            "--tasks" => options.tasks = PathBuf::from(value()?),
            "--only" => options.only = Some(value()?),
            "--timeout" => {
                options.timeout_s = value()?
                    .parse()
                    .map_err(|err| format!("--timeout: {err}"))?;
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_flags_set_the_options_and_a_percent_is_a_fraction() {
        let options = parse(args(&[
            "--out",
            "r.json",
            "--compare",
            "b.json",
            "--tolerance",
            "15",
            "--only",
            "survey",
            "--timeout",
            "60",
        ]))
        .unwrap();
        assert_eq!(options.out, Some(PathBuf::from("r.json")));
        assert_eq!(options.compare, Some(PathBuf::from("b.json")));
        assert!((options.tolerance - 0.15).abs() < 1e-12);
        assert_eq!(
            (options.only.as_deref(), options.timeout_s),
            (Some("survey"), 60)
        );
        assert_eq!(options.model, "mock");
    }

    #[test]
    fn a_bad_flag_or_value_is_refused() {
        assert!(
            parse(args(&["--nope"]))
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(
            parse(args(&["--out"]))
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(parse(args(&["--tolerance", "150"])).is_err());
        assert!(parse(args(&["--timeout", "soon"])).is_err());
    }
}

//! The guest's trust in the hub's egress CA.
//!
//! When the owner gives an agent's role an egress credential rule, the hub's egress proxy terminates
//! TLS for the rule's hosts with leaves signed by one CA and adds the credential there, so the secret
//! never reaches the guest. A guest TLS client accepts those leaves only if it trusts that CA, in
//! addition to its normal roots. The profile carries the CA (`AgentProfile::egress_ca`), and
//! [`install`] writes it where the guest's clients look:
//!
//! * [`EGRESS_CA_FILE`]: the CA alone, for clients that take one extra root (`NODE_EXTRA_CA_CERTS`);
//! * [`CA_BUNDLE_FILE`]: the system bundle plus the CA, for clients that take a whole bundle
//!   (`SSL_CERT_FILE` and friends, see [`client_variables`]);
//! * the system trust store, so a client that reads it directly trusts the CA too.
//!
//! A CA is validated before anything is written: one PEM certificate block of at most
//! [`MAX_CA_BYTES`]. Anything else is refused and the guest keeps the trust it had. A rule that is
//! removed (a profile with no CA) takes its CA back out of the system store, so a removed rule stops
//! being trusted rather than lingering in the image's roots.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::files::{Workspace, create_dirs, write_atomic};

/// The egress CA alone, as the guest recorded it (normalised by [`normalize`]).
pub const EGRESS_CA_FILE: &str = "egress-ca.pem";
/// The system bundle plus the egress CA, in the plaza's state directory.
pub const CA_BUNDLE_FILE: &str = "ca-bundle.pem";
/// The system bundles, the first that exists is the base of [`CA_BUNDLE_FILE`]: Debian and Ubuntu,
/// Alpine, then the RHEL family.
const SYSTEM_BUNDLES: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/ssl/cert.pem",
    "/etc/pki/tls/certs/ca-bundle.crt",
];
/// The system trust store the CA is appended to (the Debian layout, which the guest's initramfs has).
const SYSTEM_STORE: &str = "/etc/ssl/certs/ca-certificates.crt";
/// The largest CA accepted, as the wire carries it.
pub const MAX_CA_BYTES: usize = 16 * 1024;
const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const END: &str = "-----END CERTIFICATE-----";
const FILE_MODE: u32 = 0o644;

/// Where the system trust material lives: the guest's, or a test's scratch copies.
struct System<'a> {
    bundles: &'a [PathBuf],
    store: &'a Path,
}

/// Makes the guest trust `ca` (the profile's `egress_ca`) and nothing else new: `Some` installs or
/// replaces the CA, `None` removes the one a previous profile installed. Failures are logged, and
/// the guest keeps running on the trust it had.
pub fn install(workspace: &Workspace, ca: Option<&str>) {
    let bundles: Vec<PathBuf> = SYSTEM_BUNDLES.iter().map(PathBuf::from).collect();
    let system = System {
        bundles: &bundles,
        store: Path::new(SYSTEM_STORE),
    };
    install_at(workspace, ca, &system);
}

fn install_at(workspace: &Workspace, ca: Option<&str>, system: &System) {
    let plaza = workspace.plaza_dir();
    let recorded = fs::read_to_string(plaza.join(EGRESS_CA_FILE))
        .ok()
        .and_then(|pem| normalize(&pem));
    let Some(ca) = ca else {
        if let Some(old) = &recorded {
            retire(system, old);
        }
        remove_plaza_files(&plaza);
        return;
    };
    let Some(pem) = normalize(ca) else {
        eprintln!(
            "agent-harness: trust: refused the egress CA: it is not one PEM certificate of at most {MAX_CA_BYTES} bytes"
        );
        return;
    };
    if let Some(old) = recorded.filter(|old| *old != pem) {
        retire(system, &old);
    }
    if let Err(err) = publish(&plaza, &pem, system) {
        eprintln!("agent-harness: trust: cannot write the egress CA bundle: {err}");
    }
    append_to_store(system.store, &pem);
}

/// The validated CA as the guest records it: the one PEM certificate block (`BEGIN`, a base64 body
/// whose length is a multiple of four, `END`) with a trailing newline. `None` for anything else.
pub fn normalize(pem: &str) -> Option<String> {
    if pem.len() > MAX_CA_BYTES {
        return None;
    }
    let lines: Vec<&str> = pem.trim().lines().map(str::trim_end).collect();
    let [first, body @ .., last] = lines.as_slice() else {
        return None;
    };
    if *first != BEGIN || *last != END || body.is_empty() {
        return None;
    }
    let mut length = 0;
    for line in body {
        if !line
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        {
            return None;
        }
        length += line.len();
    }
    if length % 4 != 0 {
        return None;
    }
    Some(format!("{}\n", lines.join("\n")))
}

/// The plaza's files: the CA alone, then the system bundle plus the CA. The system bundle is read
/// before the store is touched, so the bundle never counts the CA twice.
fn publish(plaza: &Path, pem: &str, system: &System) -> io::Result<()> {
    let base = system
        .bundles
        .iter()
        .find(|path| path.is_file())
        .map(fs::read_to_string)
        .transpose()?;
    let mut bundle = base.unwrap_or_default();
    if !bundle.contains(pem) {
        if !bundle.is_empty() && !bundle.ends_with('\n') {
            bundle.push('\n');
        }
        bundle.push_str(pem);
    }
    create_dirs(plaza)?;
    write_plaza_file(&plaza.join(EGRESS_CA_FILE), pem.as_bytes())?;
    write_plaza_file(&plaza.join(CA_BUNDLE_FILE), bundle.as_bytes())
}

fn write_plaza_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))
}

/// Appends the CA to the system store unless the store already has it. A store that is absent is
/// left absent; any other failure is logged and the plaza bundle still carries the CA.
fn append_to_store(store: &Path, pem: &str) {
    let current = match fs::read_to_string(store) {
        Ok(current) => current,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(err) => {
            eprintln!("agent-harness: trust: cannot read the system store: {err}");
            return;
        }
    };
    if current.contains(pem) {
        return;
    }
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let appended = fs::OpenOptions::new()
        .append(true)
        .open(store)
        .and_then(|mut file| file.write_all(format!("{separator}{pem}").as_bytes()));
    if let Err(err) = appended {
        eprintln!("agent-harness: trust: cannot append the egress CA to the system store: {err}");
    }
}

/// Takes a CA the guest recorded back out of the system store, so a removed or replaced rule stops
/// being trusted. Only that exact text goes; the rest of the store is the image's.
fn retire(system: &System, old: &str) {
    let Ok(current) = fs::read_to_string(system.store) else {
        return;
    };
    if !current.contains(old) {
        return;
    }
    if let Err(err) = replace_system_file(system.store, current.replace(old, "").as_bytes()) {
        eprintln!("agent-harness: trust: cannot retire the egress CA from the system store: {err}");
    }
}

/// Replaces a system file by renaming a temp file over it, so a failed write leaves the old file
/// whole. Unlike [`write_atomic`] it does not hand the file to the agent: the store is the image's.
fn replace_system_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = PathBuf::from(format!("{}.plaza-tmp", path.display()));
    fs::write(&temp, bytes)?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(FILE_MODE))?;
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

fn remove_plaza_files(plaza: &Path) {
    for name in [EGRESS_CA_FILE, CA_BUNDLE_FILE] {
        match fs::remove_file(plaza.join(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("agent-harness: trust: cannot remove {name}: {err}"),
        }
    }
}

/// The variables that point the guest's TLS clients at the trust, for a command's environment. None
/// until [`install`] has written a bundle, so a guest without a rule runs with its own roots.
pub fn client_variables(workspace: &Workspace) -> Vec<(&'static str, PathBuf)> {
    let plaza = workspace.plaza_dir();
    let bundle = plaza.join(CA_BUNDLE_FILE);
    if !bundle.is_file() {
        return Vec::new();
    }
    let ca = plaza.join(EGRESS_CA_FILE);
    vec![
        ("SSL_CERT_FILE", bundle.clone()),
        ("CURL_CA_BUNDLE", bundle.clone()),
        ("REQUESTS_CA_BUNDLE", bundle.clone()),
        ("GIT_SSL_CAINFO", bundle),
        ("NODE_EXTRA_CA_CERTS", ca),
    ]
}

/// The variables for a coding engine's process, which runs as its own user and cannot read the
/// plaza's files: once [`install`] has added the egress CA, they name the system store, which holds
/// it (it is public), instead of the bundle in `.plaza/`. None before that, as for any command.
pub fn engine_variables(workspace: &Workspace) -> Vec<(&'static str, PathBuf)> {
    if !workspace.plaza_dir().join(CA_BUNDLE_FILE).is_file() {
        return Vec::new();
    }
    let store = PathBuf::from(SYSTEM_STORE);
    [
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "REQUESTS_CA_BUNDLE",
        "GIT_SSL_CAINFO",
        "NODE_EXTRA_CA_CERTS",
    ]
    .into_iter()
    .map(|key| (key, store.clone()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CA: &str = "-----BEGIN CERTIFICATE-----\nQUJD\nREVG\n-----END CERTIFICATE-----";

    #[test]
    fn an_engine_is_pointed_at_the_system_store_and_only_once_the_ca_is_installed() {
        let dir = std::env::temp_dir().join(format!("apz-trust-engine-{}", std::process::id()));
        let workspace = Workspace::at(dir.clone());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(workspace.plaza_dir()).expect("the plaza folder");
        assert!(
            engine_variables(&workspace).is_empty(),
            "no bundle, no variables"
        );
        fs::write(workspace.plaza_dir().join(CA_BUNDLE_FILE), "pem").expect("a bundle");
        let vars = engine_variables(&workspace);
        assert_eq!(vars.len(), 5);
        assert!(vars.iter().all(|(_, path)| path == Path::new(SYSTEM_STORE)));
        assert!(vars.iter().any(|(key, _)| *key == "NODE_EXTRA_CA_CERTS"));
        let _ = fs::remove_dir_all(&dir);
    }
    const OTHER_CA: &str = "-----BEGIN CERTIFICATE-----\nR0hJ\n-----END CERTIFICATE-----\n";

    /// A scratch workspace and system material: a missing first bundle, a present second bundle
    /// and a store that already holds the image's root.
    struct Fixture {
        dir: PathBuf,
        workspace: Workspace,
        bundles: Vec<PathBuf>,
        store: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("apz-harness-trust-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a scratch directory");
            let workspace = Workspace::at(dir.join("workspace"));
            let bundles = vec![dir.join("missing.pem"), dir.join("system.pem")];
            fs::write(&bundles[1], "SYSTEM ROOT\n").expect("a system bundle");
            let store = dir.join("store.crt");
            fs::write(&store, "STORE ROOT\n").expect("a system store");
            Self {
                dir,
                workspace,
                bundles,
                store,
            }
        }

        fn system(&self) -> System<'_> {
            System {
                bundles: &self.bundles,
                store: &self.store,
            }
        }

        fn read(&self, path: PathBuf) -> String {
            fs::read_to_string(path).expect("a file the install wrote")
        }

        fn plaza(&self, name: &str) -> PathBuf {
            self.workspace.plaza_dir().join(name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_single_certificate_block_is_accepted_and_normalised() {
        assert_eq!(normalize(CA), Some(format!("{CA}\n")));
        assert_eq!(normalize(&format!("\n  {CA}\n\n")), Some(format!("{CA}\n")));
        assert_eq!(normalize(OTHER_CA), Some(OTHER_CA.to_string()));
    }

    #[test]
    fn anything_else_is_refused() {
        let oversized = format!("{BEGIN}\n{}{END}", "QUJD\n".repeat(4000));
        let shell = format!("{BEGIN}\n$(reboot)\n{END}");
        let two_blocks = format!("{CA}\n{CA}");
        let bad_length = format!("{BEGIN}\nQUJDR\n{END}");
        let no_end = format!("{BEGIN}\nQUJD\n");
        let empty_body = format!("{BEGIN}\n{END}");
        let refused: Vec<&str> = vec![
            "",
            "not a certificate",
            oversized.as_str(),
            shell.as_str(),
            two_blocks.as_str(),
            bad_length.as_str(),
            no_end.as_str(),
            empty_body.as_str(),
        ];
        for input in refused {
            assert_eq!(normalize(input), None, "{input:.40}");
        }
    }

    #[test]
    fn install_writes_the_ca_bundle_and_the_store_and_replaces_them() {
        let fixture = Fixture::new("install");
        install_at(&fixture.workspace, Some(CA), &fixture.system());

        assert_eq!(
            fixture.read(fixture.plaza(EGRESS_CA_FILE)),
            format!("{CA}\n")
        );
        assert_eq!(
            fixture.read(fixture.plaza(CA_BUNDLE_FILE)),
            format!("SYSTEM ROOT\n{CA}\n")
        );
        assert_eq!(
            fixture.read(fixture.store.clone()),
            format!("STORE ROOT\n{CA}\n")
        );
        for name in [EGRESS_CA_FILE, CA_BUNDLE_FILE] {
            let mode = fs::metadata(fixture.plaza(name))
                .expect("a plaza file")
                .permissions();
            assert_eq!(mode.mode() & 0o777, FILE_MODE, "{name}");
        }

        install_at(&fixture.workspace, Some(CA), &fixture.system());
        assert_eq!(
            fixture.read(fixture.store.clone()).matches(CA).count(),
            1,
            "the same CA is not appended twice"
        );

        install_at(&fixture.workspace, Some(OTHER_CA), &fixture.system());
        let store = fixture.read(fixture.store.clone());
        assert!(!store.contains(CA), "a replaced CA is retired: {store}");
        assert!(
            store.contains(OTHER_CA) && store.starts_with("STORE ROOT\n"),
            "{store}"
        );
        assert_eq!(fixture.read(fixture.plaza(EGRESS_CA_FILE)), OTHER_CA);
    }

    #[test]
    fn removing_the_rule_takes_its_ca_back_out_and_keeps_the_image_roots() {
        let fixture = Fixture::new("remove");
        install_at(&fixture.workspace, Some(CA), &fixture.system());
        install_at(&fixture.workspace, None, &fixture.system());

        assert!(!fixture.plaza(EGRESS_CA_FILE).exists());
        assert!(!fixture.plaza(CA_BUNDLE_FILE).exists());
        assert_eq!(fixture.read(fixture.store.clone()), "STORE ROOT\n");
    }

    #[test]
    fn with_no_system_bundle_the_plaza_bundle_is_the_ca_alone_and_no_store_appears() {
        let fixture = Fixture::new("bare");
        let no_bundles = vec![fixture.dir.join("absent.pem")];
        let no_store = fixture.dir.join("absent-store.crt");
        let system = System {
            bundles: &no_bundles,
            store: &no_store,
        };
        install_at(&fixture.workspace, Some(CA), &system);

        assert_eq!(
            fixture.read(fixture.plaza(CA_BUNDLE_FILE)),
            format!("{CA}\n")
        );
        assert!(!no_store.exists(), "an absent store is not created");
    }

    #[test]
    fn a_refused_ca_leaves_the_installed_trust_alone() {
        let fixture = Fixture::new("refused");
        install_at(&fixture.workspace, Some(CA), &fixture.system());
        install_at(&fixture.workspace, Some("garbage"), &fixture.system());

        assert_eq!(
            fixture.read(fixture.plaza(EGRESS_CA_FILE)),
            format!("{CA}\n")
        );
        assert!(fixture.read(fixture.store.clone()).contains(CA));
    }

    #[test]
    fn the_client_variables_appear_only_with_a_bundle() {
        let fixture = Fixture::new("variables");
        assert!(client_variables(&fixture.workspace).is_empty());

        install_at(&fixture.workspace, Some(CA), &fixture.system());
        let variables = client_variables(&fixture.workspace);
        let bundle = fixture.plaza(CA_BUNDLE_FILE);
        let keys: Vec<&str> = variables.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            [
                "SSL_CERT_FILE",
                "CURL_CA_BUNDLE",
                "REQUESTS_CA_BUNDLE",
                "GIT_SSL_CAINFO",
                "NODE_EXTRA_CA_CERTS"
            ]
        );
        assert_eq!(variables[0].1, bundle);
        assert_eq!(variables[4].1, fixture.plaza(EGRESS_CA_FILE));
    }
}

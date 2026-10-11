//! Model providers (`providers.json`), their credentials (`secrets.json`,
//! 0600, write-only through the API) and the gateway built from them.
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use apz_gateway::Gateway;
use apz_http::Router;

use crate::state::App;

pub struct Store {
    dir: PathBuf,
    env: BTreeMap<String, String>,
    gateway: Gateway,
}

impl Store {
    /// Loads providers and secrets; `env` is the env-file's variables (looked
    /// up before the process environment).
    pub fn load(state_dir: &Path, env: BTreeMap<String, String>) -> std::io::Result<Self> {
        // contract — TODO(providers slice): providers.json with defaults, secrets.json
        let gateway = Gateway::new(Vec::new(), BTreeMap::<String, String>::new());
        Ok(Self { dir: state_dir.to_path_buf(), env, gateway })
    }

    /// The gateway over the current providers and secrets.
    pub fn gateway(&self) -> Gateway {
        // contract
        let _ = (&self.dir, &self.env);
        self.gateway.clone()
    }
}

/// `/v1/provider-kinds`, `/v1/providers…`, `/v1/models`.
pub fn routes(router: Router<App>) -> Router<App> {
    // TODO(providers slice)
    router
}

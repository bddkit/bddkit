use crate::db::DbHandle;
use crate::http::{Apis, HttpState};
use crate::options::{Options, OptionsLayer};
use crate::unique::Generator;
use crate::vars::VarStack;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One directory per feature file, for files a run produces: an object
/// downloaded by a plugin step, or a response body saved by a host step. Its
/// point is that `report.pdf` in two feature files is two files.
///
/// The counter is process-global for the same reason the artifacts counter is:
/// two workers handed one path would overwrite each other's work.
static WORKSPACE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Workspace {
    path: PathBuf,
    created: bool,
}

impl Workspace {
    fn new(run_id: &str) -> Self {
        let index = WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir()
                .join(format!("bddkit-{run_id}"))
                .join("workspace")
                .join(format!("{index:06}")),
            created: false,
        }
    }

    /// Created by the host on first use — deliberately unlike `artifacts_dir`,
    /// which the host allocates but never creates. That one is fresh on every
    /// dispatch and most dispatches write nothing, so a `mkdir` per call would
    /// be waste; this one is a single directory per file, shared by whoever
    /// writes into it.
    ///
    /// Nothing deletes it. A failed run's downloaded files stay inspectable,
    /// and there is no teardown path to get wrong on a file that panicked.
    pub fn dir(&mut self) -> Result<&Path, String> {
        if !self.created {
            std::fs::create_dir_all(&self.path).map_err(|e| {
                format!(
                    "failed to create the workspace {}: {e}",
                    self.path.display()
                )
            })?;
            self.created = true;
        }
        Ok(&self.path)
    }
}

/// Variables live for the feature file; HTTP state, the current DB connection,
/// and debug mode live for the scenario. Asymmetry from Behat's behavior (spec §3).
pub struct World {
    pub vars: VarStack,
    pub http: HttpState,
    pub db: DbHandle,
    pub debug: bool,
    pub generator: Arc<Generator>,
    pub options: Options,
    pending_options: Option<OptionsLayer>,
    /// Run-wide configuration, not scenario state: `reset_scenario`
    /// does not touch it.
    pub srp: Option<Arc<crate::srp::SrpParams>>,
    /// Scenario state: which instance of each plugin group is selected.
    pub plugins: crate::plugin::PluginState,
    /// Per feature file, like `vars`: `reset_scenario` does not touch it.
    workspace: Workspace,
}

impl World {
    pub fn new(
        apis: Arc<Apis>,
        generator: Arc<Generator>,
        db: DbHandle,
        srp: Option<Arc<crate::srp::SrpParams>>,
        plugins: Option<Arc<crate::plugin::Plugins>>,
        options: Options,
    ) -> Self {
        let workspace = Workspace::new(generator.run_id());
        Self {
            vars: VarStack::new(),
            http: HttpState::new(apis),
            db,
            debug: false,
            generator,
            srp,
            options,
            pending_options: None,
            plugins: crate::plugin::PluginState::new(plugins),
            workspace,
        }
    }

    /// The working directory of this feature file, created on first use.
    pub fn workspace_dir(&mut self) -> Result<&Path, String> {
        self.workspace.dir()
    }

    pub fn arm_options(&mut self, layer: OptionsLayer) {
        self.pending_options = Some(layer);
    }

    pub fn take_options(&mut self) -> Option<OptionsLayer> {
        self.pending_options.take()
    }

    /// At the scenario boundary: the request, connection, and debug flag reset; variables remain.
    pub fn reset_scenario(&mut self) {
        self.http.reset();
        self.db.reset();
        self.debug = false;
        self.pending_options = None;
        self.plugins.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::http::ApiResource;
    use crate::unique::Generator;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn apis() -> Arc<Apis> {
        let mut by_name = HashMap::new();
        by_name.insert(
            "main".to_string(),
            ApiResource::new("http://x.local", 5, Vec::new(), Options::default())
                .expect("valid base_url"),
        );
        Arc::new(Apis::new(by_name, Some("main".to_string())).expect("default declared"))
    }

    #[test]
    fn reset_scenario_clears_debug() {
        let mut w = World::new(
            apis(),
            Arc::new(Generator::new()),
            DbHandle::new(None, String::new()),
            None,
            None,
            Options::default(),
        );
        w.debug = true;
        w.reset_scenario();
        assert!(!w.debug, "debug resets at the scenario boundary");
    }

    #[test]
    fn the_current_plugin_instance_resets_at_the_scenario_boundary() {
        // Invariant 2: a scenario never inherits the instance another one switched
        // to, exactly as the current API resource resets.
        let mut state = crate::plugin::PluginState::new(None);
        state.set_defaults(
            [("echo".to_string(), "a".to_string())]
                .into_iter()
                .collect(),
        );
        state.use_instance_unchecked("echo", "b");
        assert_eq!(state.current("echo").expect("selected"), "b");
        state.reset();
        assert_eq!(state.current("echo").expect("selected"), "a");
    }

    #[test]
    fn reset_scenario_resets_the_plugin_selection() {
        let mut w = World::new(
            apis(),
            Arc::new(Generator::new()),
            DbHandle::new(None, String::new()),
            None,
            None,
            Options::default(),
        );
        w.plugins.set_defaults(
            [("echo".to_string(), "a".to_string())]
                .into_iter()
                .collect(),
        );
        w.plugins.use_instance_unchecked("echo", "b");
        w.reset_scenario();
        assert_eq!(w.plugins.current("echo").expect("selected"), "a");
    }

    #[test]
    fn srp_parameters_survive_a_scenario_boundary() {
        let params = Arc::new(crate::srp::SrpParams {
            variant: crate::srp::Variant::HexString,
            prime: num_bigint::BigUint::from(23u32),
            generator: num_bigint::BigUint::from(5u32),
            hash: crate::srp::HashAlg::Sha256,
        });
        let mut w = World::new(
            apis(),
            Arc::new(Generator::new()),
            DbHandle::new(None, String::new()),
            Some(params),
            None,
            Options::default(),
        );
        w.reset_scenario();
        assert!(
            w.srp.is_some(),
            "SRP parameters are run-wide configuration, not scenario state"
        );
    }

    #[test]
    fn allocating_a_workspace_does_not_touch_the_disk() {
        let ws = Workspace::new("test-run-alloc");
        assert!(
            !ws.path.exists(),
            "a file that never asks for the path must not leave a directory behind"
        );
    }

    #[test]
    fn asking_for_the_workspace_creates_it_and_the_path_never_moves() {
        let mut ws = Workspace::new("test-run-create");
        let first = ws.dir().expect("create").to_path_buf();
        assert!(
            first.exists(),
            "the host creates the workspace, not the plugin"
        );
        let second = ws.dir().expect("create again").to_path_buf();
        assert_eq!(
            first, second,
            "a second call must not allocate a new directory"
        );
        let _ = std::fs::remove_dir_all(&first);
    }

    #[test]
    fn two_workspaces_in_one_run_never_share_a_directory() {
        let a = Workspace::new("test-run-collide");
        let b = Workspace::new("test-run-collide");
        assert_ne!(
            a.path, b.path,
            "two feature files must not resolve `report.pdf` to the same file"
        );
    }
}

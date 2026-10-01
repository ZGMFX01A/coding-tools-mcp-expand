//! Share command handles and retry ledgers between MCP and Actions for the same runtime policy.
use super::{reliability::Reliability, session::SessionStore};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock, Weak},
};
pub struct RuntimeState {
    pub reliability: Arc<Reliability>,
    pub sessions: Arc<SessionStore>,
    pub mutation_lock: Arc<Mutex<()>>,
}
type RuntimeKey = (std::path::PathBuf, String, String);
static RUNTIMES: OnceLock<Mutex<HashMap<RuntimeKey, Weak<RuntimeState>>>> = OnceLock::new();
static LOCKS: OnceLock<Mutex<HashMap<std::path::PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
pub fn get(root: &Path, permission: &str, profile: &str) -> Arc<RuntimeState> {
    let key = (
        root.to_path_buf(),
        permission.into(),
        super::registry::normalize_tool_profile(profile).into(),
    );
    let mut runtimes = RUNTIMES
        .get_or_init(Default::default)
        .lock()
        .expect("runtime lock");
    runtimes.retain(|_, state| state.strong_count() > 0);
    if let Some(state) = runtimes.get(&key).and_then(Weak::upgrade) {
        return state;
    }
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("workspace locks");
    locks.retain(|_, lock| lock.strong_count() > 0);
    let mutation_lock = locks.get(root).and_then(Weak::upgrade).unwrap_or_else(|| {
        let lock = Arc::new(Mutex::new(()));
        locks.insert(root.into(), Arc::downgrade(&lock));
        lock
    });
    let state = Arc::new(RuntimeState {
        reliability: Arc::new(Reliability::default()),
        sessions: Arc::new(SessionStore::new()),
        mutation_lock,
    });
    runtimes.insert(key, Arc::downgrade(&state));
    state
}

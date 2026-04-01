//! Sled-backed persistence for research strategies.
//!
//! Stores [`ResearchStrategy`] metadata in sled and compiled artifacts
//! on the filesystem. Provides CRUD operations for strategy lifecycle
//! management.

use std::path::{Path, PathBuf};

use rara_domain::research::{ResearchStrategy, ResearchStrategyStatus};
use snafu::{ResultExt, Snafu};
use uuid::Uuid;

/// Errors from strategy store operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StrategyStoreError {
    /// sled database operation failed.
    #[snafu(display("sled error: {source}"))]
    Sled { source: sled::Error },

    /// JSON serialization/deserialization failed.
    #[snafu(display("serialization error: {source}"))]
    Serialize { source: serde_json::Error },

    /// Filesystem I/O failed.
    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },

    /// Strategy not found.
    #[snafu(display("strategy {id} not found"))]
    NotFound { id: Uuid },
}

/// Alias for strategy store results.
pub type Result<T> = std::result::Result<T, StrategyStoreError>;

/// Sled-backed store for research strategy metadata and artifacts.
pub struct StrategyStore {
    /// sled tree for strategy metadata (id -> JSON).
    strategies:   sled::Tree,
    /// Filesystem directory for compiled artifacts.
    artifact_dir: PathBuf,
}

impl StrategyStore {
    /// Open or create a strategy store with a shared sled database.
    ///
    /// Uses the given sled database (shared with other stores) and a dedicated
    /// filesystem directory for binary artifacts.
    pub fn open(db: &sled::Db, artifact_dir: &Path) -> Result<Self> {
        let strategies = db.open_tree("research_strategies").context(SledSnafu)?;
        std::fs::create_dir_all(artifact_dir).context(IoSnafu)?;
        Ok(Self {
            strategies,
            artifact_dir: artifact_dir.to_owned(),
        })
    }

    /// Open or create a strategy store at a filesystem path.
    ///
    /// Opens its own sled database at `db_path` and stores artifacts in
    /// `artifact_dir`. Convenient when a shared database is not needed.
    pub fn open_path(db_path: &Path, artifact_dir: &Path) -> Result<Self> {
        let db = sled::open(db_path).context(SledSnafu)?;
        Self::open(&db, artifact_dir)
    }

    /// Save a research strategy record.
    pub fn save(&self, strategy: &ResearchStrategy) -> Result<()> {
        let key = strategy.id.as_bytes();
        let value = serde_json::to_vec(strategy).context(SerializeSnafu)?;
        self.strategies.insert(key, value).context(SledSnafu)?;
        Ok(())
    }

    /// Get a strategy by ID.
    pub fn get(&self, id: Uuid) -> Result<Option<ResearchStrategy>> {
        self.strategies
            .get(id.as_bytes())
            .context(SledSnafu)?
            .map(|bytes| serde_json::from_slice(&bytes).context(SerializeSnafu))
            .transpose()
    }

    /// Update strategy status.
    pub fn update_status(&self, id: Uuid, status: ResearchStrategyStatus) -> Result<()> {
        let mut strategy = self.get(id)?.ok_or(StrategyStoreError::NotFound { id })?;
        strategy.status = status;
        self.save(&strategy)
    }

    /// Save a compiled artifact to the filesystem.
    pub fn save_artifact(&self, id: Uuid, artifact: &[u8]) -> Result<()> {
        let path = self.artifact_path(id);
        std::fs::write(&path, artifact).context(IoSnafu)
    }

    /// Load a compiled artifact from the filesystem.
    pub fn load_artifact(&self, id: Uuid) -> Result<Vec<u8>> {
        let path = self.artifact_path(id);
        std::fs::read(&path).context(IoSnafu)
    }

    /// List all strategies, optionally filtered by status.
    pub fn list(
        &self,
        status_filter: Option<ResearchStrategyStatus>,
    ) -> Result<Vec<ResearchStrategy>> {
        self.strategies
            .iter()
            .map(|item| {
                let (_, bytes) = item.context(SledSnafu)?;
                serde_json::from_slice::<ResearchStrategy>(&bytes).context(SerializeSnafu)
            })
            .filter(|s| match (&status_filter, s) {
                (Some(filter), Ok(strategy)) => strategy.status == *filter,
                _ => true,
            })
            .collect()
    }

    /// Filesystem path for a strategy artifact.
    fn artifact_path(&self, id: Uuid) -> PathBuf {
        self.artifact_dir.join(format!("{id}.artifact"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_get_and_update_status() {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path().join("db")).unwrap();
        let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

        let strategy = ResearchStrategy::builder()
            .hypothesis_id(Uuid::new_v4())
            .source_code("fn run() {}")
            .build();

        store.save(&strategy).unwrap();

        let loaded = store.get(strategy.id).unwrap().unwrap();
        assert_eq!(loaded.id, strategy.id);
        assert_eq!(loaded.source_code, "fn run() {}");
        assert_eq!(loaded.status, ResearchStrategyStatus::Compiled);

        store
            .update_status(strategy.id, ResearchStrategyStatus::Accepted)
            .unwrap();
        let updated = store.get(strategy.id).unwrap().unwrap();
        assert_eq!(updated.status, ResearchStrategyStatus::Accepted);
    }

    #[test]
    fn save_and_load_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path().join("db")).unwrap();
        let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

        let id = Uuid::new_v4();
        let artifact = b"fake wasm bytes";
        store.save_artifact(id, artifact).unwrap();

        let loaded = store.load_artifact(id).unwrap();
        assert_eq!(loaded, artifact);
    }

    #[test]
    fn list_filters_by_status() {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path().join("db")).unwrap();
        let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

        let s1 = ResearchStrategy::builder()
            .hypothesis_id(Uuid::new_v4())
            .source_code("s1")
            .build();
        let mut s2 = ResearchStrategy::builder()
            .hypothesis_id(Uuid::new_v4())
            .source_code("s2")
            .build();
        s2.status = ResearchStrategyStatus::Accepted;

        store.save(&s1).unwrap();
        store.save(&s2).unwrap();

        let compiled = store.list(Some(ResearchStrategyStatus::Compiled)).unwrap();
        assert_eq!(compiled.len(), 1);

        let all = store.list(None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path().join("db")).unwrap();
        let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

        assert!(store.get(Uuid::new_v4()).unwrap().is_none());
    }
}

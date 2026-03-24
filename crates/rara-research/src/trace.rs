//! DAG storage for research hypotheses, experiments, and feedback using sled.

use std::path::Path;

use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use rara_domain::research::{Experiment, Hypothesis, HypothesisFeedback};

/// Errors that can occur in trace storage.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum TraceError {
    /// A sled database error.
    #[snafu(display("sled error: {source}"))]
    Sled {
        /// The underlying sled error.
        source: sled::Error,
    },
    /// A JSON serialization/deserialization error.
    #[snafu(display("serialization error: {source}"))]
    Serialize {
        /// The underlying `serde_json` error.
        source: serde_json::Error,
    },
}

/// Alias for results from trace operations.
pub type Result<T> = std::result::Result<T, TraceError>;

/// DAG storage for the research loop, persisting hypotheses, experiments,
/// and feedback in sled trees.
pub struct Trace {
    hypotheses: sled::Tree,
    experiments: sled::Tree,
    feedbacks: sled::Tree,
}

impl Trace {
    /// Open (or create) a trace store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let db = sled::open(path).context(SledSnafu)?;
        let hypotheses = db.open_tree("hypotheses").context(SledSnafu)?;
        let experiments = db.open_tree("experiments").context(SledSnafu)?;
        let feedbacks = db.open_tree("feedbacks").context(SledSnafu)?;
        Ok(Self {
            hypotheses,
            experiments,
            feedbacks,
        })
    }

    /// Persist a hypothesis.
    pub fn save_hypothesis(&self, h: &Hypothesis) -> Result<()> {
        let json = serde_json::to_vec(h).context(SerializeSnafu)?;
        self.hypotheses
            .insert(h.id().as_bytes(), json)
            .context(SledSnafu)?;
        Ok(())
    }

    /// Retrieve a hypothesis by ID.
    pub fn get_hypothesis(&self, id: Uuid) -> Result<Option<Hypothesis>> {
        self.hypotheses
            .get(id.as_bytes())
            .context(SledSnafu)?
            .map(|bytes| serde_json::from_slice(&bytes).context(SerializeSnafu))
            .transpose()
    }

    /// Walk parent links from a hypothesis to the root, returning the full
    /// ancestor chain (starting with the given hypothesis).
    pub fn ancestor_chain(&self, id: Uuid) -> Result<Vec<Hypothesis>> {
        let mut chain = Vec::new();
        let mut current_id = Some(id);

        while let Some(cid) = current_id {
            let Some(h) = self.get_hypothesis(cid)? else {
                break;
            };
            current_id = h.parent();
            chain.push(h);
        }

        Ok(chain)
    }

    /// Persist an experiment.
    pub fn save_experiment(&self, exp: &Experiment) -> Result<()> {
        let json = serde_json::to_vec(exp).context(SerializeSnafu)?;
        self.experiments
            .insert(exp.id().as_bytes(), json)
            .context(SledSnafu)?;
        Ok(())
    }

    /// Retrieve an experiment by ID.
    pub fn get_experiment(&self, id: Uuid) -> Result<Option<Experiment>> {
        self.experiments
            .get(id.as_bytes())
            .context(SledSnafu)?
            .map(|bytes| serde_json::from_slice(&bytes).context(SerializeSnafu))
            .transpose()
    }

    /// Persist a hypothesis feedback entry.
    pub fn save_feedback(&self, fb: &HypothesisFeedback) -> Result<()> {
        let json = serde_json::to_vec(fb).context(SerializeSnafu)?;
        // Key: "{experiment_id}/{created_at}" for multiple feedbacks per experiment
        let key = format!("{}/{}", fb.experiment_id(), fb.reason());
        self.feedbacks
            .insert(key.as_bytes(), json)
            .context(SledSnafu)?;
        Ok(())
    }

    /// Get all feedback entries for a given experiment.
    pub fn get_feedback_for_experiment(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<HypothesisFeedback>> {
        let prefix = experiment_id.to_string();
        self.feedbacks
            .scan_prefix(prefix.as_bytes())
            .map(|res| {
                let (_, bytes) = res.context(SledSnafu)?;
                serde_json::from_slice(&bytes).context(SerializeSnafu)
            })
            .collect()
    }

    /// Find the experiment with the best accepted feedback.
    ///
    /// Scans all feedbacks and returns the first accepted experiment found,
    /// along with its feedback. Returns `None` if no accepted experiments exist.
    pub fn get_best_experiment(&self) -> Result<Option<(Experiment, HypothesisFeedback)>> {
        let mut best: Option<(Experiment, HypothesisFeedback)> = None;

        for res in &self.feedbacks {
            let (_, bytes) = res.context(SledSnafu)?;
            let fb: HypothesisFeedback =
                serde_json::from_slice(&bytes).context(SerializeSnafu)?;

            if !fb.decision() {
                continue;
            }

            let Some(exp) = self.get_experiment(fb.experiment_id())? else {
                continue;
            };

            // Pick the first accepted; could be extended with sharpe comparison
            if best.is_none() {
                best = Some((exp, fb));
            }
        }

        Ok(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hypothesis(text: &str, parent: Option<Uuid>) -> Hypothesis {
        Hypothesis::builder()
            .text(text)
            .reason("test reason")
            .maybe_parent(parent)
            .build()
    }

    #[test]
    fn ancestor_chain_three_levels() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let root = make_hypothesis("root", None);
        let mid = make_hypothesis("mid", Some(root.id()));
        let leaf = make_hypothesis("leaf", Some(mid.id()));

        trace.save_hypothesis(&root).unwrap();
        trace.save_hypothesis(&mid).unwrap();
        trace.save_hypothesis(&leaf).unwrap();

        let chain = trace.ancestor_chain(leaf.id()).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].id(), leaf.id());
        assert_eq!(chain[1].id(), mid.id());
        assert_eq!(chain[2].id(), root.id());
    }

    #[test]
    fn get_best_experiment_returns_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let h = make_hypothesis("test", None);
        trace.save_hypothesis(&h).unwrap();

        // Create rejected experiment
        let rejected_exp = Experiment::builder()
            .hypothesis_id(h.id())
            .strategy_code("bad code")
            .build();
        trace.save_experiment(&rejected_exp).unwrap();

        let rejected_fb = HypothesisFeedback::builder()
            .experiment_id(rejected_exp.id())
            .decision(false)
            .reason("poor performance")
            .observations("low sharpe")
            .build();
        trace.save_feedback(&rejected_fb).unwrap();

        // Create accepted experiment
        let accepted_exp = Experiment::builder()
            .hypothesis_id(h.id())
            .strategy_code("good code")
            .build();
        trace.save_experiment(&accepted_exp).unwrap();

        let accepted_fb = HypothesisFeedback::builder()
            .experiment_id(accepted_exp.id())
            .decision(true)
            .reason("strong performance")
            .observations("high sharpe")
            .build();
        trace.save_feedback(&accepted_fb).unwrap();

        let best = trace.get_best_experiment().unwrap();
        assert!(best.is_some());
        let (exp, fb) = best.unwrap();
        assert!(fb.decision());
        assert_eq!(exp.id(), accepted_exp.id());
    }
}

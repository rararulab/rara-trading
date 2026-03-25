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

/// Selects how a new DAG node relates to existing nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagSelection {
    /// Start a new root with no parents.
    NewRoot,
    /// Attach to the most recently recorded node.
    Latest,
    /// Attach to a specific node by its index.
    Specific(u64),
}

/// DAG storage for the research loop, persisting hypotheses, experiments,
/// and feedback in sled trees.
pub struct Trace {
    hypotheses: sled::Tree,
    experiments: sled::Tree,
    feedbacks: sled::Tree,
    /// Maps `u64_be_bytes` node index to `serde_json Vec<u64>` of parent indices.
    dag_parents: sled::Tree,
    /// Maps `u64_be_bytes` sequential index to experiment ID bytes.
    hist_order: sled::Tree,
}

impl Trace {
    /// Open (or create) a trace store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let db = sled::open(path).context(SledSnafu)?;
        let hypotheses = db.open_tree("hypotheses").context(SledSnafu)?;
        let experiments = db.open_tree("experiments").context(SledSnafu)?;
        let feedbacks = db.open_tree("feedbacks").context(SledSnafu)?;
        let dag_parents = db.open_tree("dag_parents").context(SledSnafu)?;
        let hist_order = db.open_tree("hist_order").context(SledSnafu)?;
        Ok(Self {
            hypotheses,
            experiments,
            feedbacks,
            dag_parents,
            hist_order,
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

    // --- DAG methods ---

    /// Return the next auto-increment index for `hist_order`.
    fn next_index(&self) -> Result<u64> {
        // The last key in hist_order (ordered by big-endian u64) is the highest index
        Ok(self
            .hist_order
            .last()
            .context(SledSnafu)?
            .map_or(0, |(k, _)| {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&k);
                u64::from_be_bytes(buf) + 1
            }))
    }

    /// Record an experiment and its feedback as a new DAG node.
    ///
    /// Saves the experiment and feedback, assigns a sequential index in
    /// `hist_order`, and links parent(s) in `dag_parents` based on the
    /// `DagSelection`. Returns the new node index.
    pub fn record(
        &self,
        exp: &Experiment,
        feedback: &HypothesisFeedback,
        parent: &DagSelection,
    ) -> Result<u64> {
        self.save_experiment(exp)?;
        self.save_feedback(feedback)?;

        let idx = self.next_index()?;
        let idx_bytes = idx.to_be_bytes();

        // Store experiment ID in hist_order
        self.hist_order
            .insert(idx_bytes, exp.id().as_bytes().as_slice())
            .context(SledSnafu)?;

        // Determine parent indices
        let parents: Vec<u64> = match parent {
            DagSelection::NewRoot => vec![],
            DagSelection::Latest => {
                if idx > 0 {
                    vec![idx - 1]
                } else {
                    vec![]
                }
            }
            DagSelection::Specific(p) => vec![*p],
        };

        let parents_json = serde_json::to_vec(&parents).context(SerializeSnafu)?;
        self.dag_parents
            .insert(idx_bytes, parents_json)
            .context(SledSnafu)?;

        Ok(idx)
    }

    /// Retrieve the experiment and feedback for a given node index.
    fn get_node(&self, node_idx: u64) -> Result<Option<(Experiment, HypothesisFeedback)>> {
        let idx_bytes = node_idx.to_be_bytes();

        let Some(exp_id_bytes) = self.hist_order.get(idx_bytes).context(SledSnafu)? else {
            return Ok(None);
        };

        let exp_id = Uuid::from_slice(&exp_id_bytes).unwrap_or_default();
        let Some(exp) = self.get_experiment(exp_id)? else {
            return Ok(None);
        };

        // Return the first feedback for this experiment
        let fbs = self.get_feedback_for_experiment(exp_id)?;
        let Some(fb) = fbs.into_iter().next() else {
            return Ok(None);
        };

        Ok(Some((exp, fb)))
    }

    /// Get parent indices for a node.
    fn get_parent_indices(&self, node_idx: u64) -> Result<Vec<u64>> {
        let idx_bytes = node_idx.to_be_bytes();
        self.dag_parents
            .get(idx_bytes)
            .context(SledSnafu)?
            .map(|bytes| serde_json::from_slice::<Vec<u64>>(&bytes).context(SerializeSnafu))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Find the state-of-the-art: the accepted experiment with the highest
    /// Sharpe ratio across all recorded feedback.
    pub fn get_sota(&self) -> Result<Option<(Experiment, HypothesisFeedback)>> {
        let mut best: Option<(Experiment, HypothesisFeedback, f64)> = None;

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

            let sharpe = exp
                .backtest_result()
                .map_or(f64::NEG_INFINITY, rara_domain::research::BacktestResult::sharpe_ratio);

            let dominated = best.as_ref().is_some_and(|(_, _, best_sharpe)| *best_sharpe >= sharpe);
            if !dominated {
                best = Some((exp, fb, sharpe));
            }
        }

        Ok(best.map(|(exp, fb, _)| (exp, fb)))
    }

    /// Walk the DAG parent chain from a node, collecting all ancestors.
    ///
    /// Returns pairs of `(Experiment, HypothesisFeedback)` starting from the
    /// given node and walking toward roots. Follows the first parent at each
    /// step for a linear chain.
    pub fn ancestors(&self, node_idx: u64) -> Result<Vec<(Experiment, HypothesisFeedback)>> {
        let mut result = Vec::new();
        let mut current = Some(node_idx);

        while let Some(idx) = current {
            let Some(pair) = self.get_node(idx)? else {
                break;
            };
            result.push(pair);

            let parents = self.get_parent_indices(idx)?;
            // Follow the first parent for linear chain traversal
            current = parents.into_iter().next();
        }

        Ok(result)
    }

    /// Find all direct children of a given parent node.
    ///
    /// Scans all `dag_parents` entries and returns nodes whose parent list
    /// contains `parent_idx`.
    pub fn children(
        &self,
        parent_idx: u64,
    ) -> Result<Vec<(Experiment, HypothesisFeedback)>> {
        self.dag_parents
            .iter()
            .filter_map(|res| {
                let (key_bytes, val_bytes) = match res {
                    Ok(pair) => pair,
                    Err(e) => return Some(Err(TraceError::Sled { source: e })),
                };

                let parents: Vec<u64> = match serde_json::from_slice(&val_bytes) {
                    Ok(p) => p,
                    Err(e) => return Some(Err(TraceError::Serialize { source: e })),
                };

                if !parents.contains(&parent_idx) {
                    return None;
                }

                let mut buf = [0u8; 8];
                buf.copy_from_slice(&key_bytes);
                let child_idx = u64::from_be_bytes(buf);

                match self.get_node(child_idx) {
                    Ok(Some(pair)) => Some(Ok(pair)),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .collect()
    }

    /// Render trace history as structured text for LLM prompt injection.
    ///
    /// Shows the most recent `max_entries` entries from `hist_order`, formatted
    /// as one line per iteration with hypothesis, result, metrics, and feedback.
    pub fn format_for_prompt(&self, max_entries: usize) -> Result<String> {
        let total = self.hist_order.len();
        let skip = total.saturating_sub(max_entries);

        let lines: Vec<String> = self
            .hist_order
            .iter()
            .skip(skip)
            .enumerate()
            .filter_map(|(i, res)| {
                let (_, exp_id_bytes) = match res {
                    Ok(pair) => pair,
                    Err(e) => return Some(Err(TraceError::Sled { source: e })),
                };

                let exp_id = Uuid::from_slice(&exp_id_bytes).unwrap_or_default();

                let exp = match self.get_experiment(exp_id) {
                    Ok(Some(e)) => e,
                    Ok(None) => return None,
                    Err(e) => return Some(Err(e)),
                };

                let fbs = match self.get_feedback_for_experiment(exp_id) {
                    Ok(f) => f,
                    Err(e) => return Some(Err(e)),
                };

                let fb = fbs.into_iter().next()?;

                // Look up the hypothesis text
                let hyp_text = match self.get_hypothesis(exp.hypothesis_id()) {
                    Ok(Some(h)) => h.text().to_owned(),
                    _ => "unknown".to_owned(),
                };

                let decision_str = if fb.decision() { "accepted" } else { "rejected" };

                let (sharpe, pnl) = exp.backtest_result().map_or_else(
                    || ("N/A".to_owned(), "N/A".to_owned()),
                    |br| (format!("{:.2}", br.sharpe_ratio()), format!("{:.2}", br.pnl())),
                );

                let iteration = skip + i;

                Some(Ok(format!(
                    "[Iteration {iteration}] Hypothesis: {hyp_text} | Result: {decision_str} | Sharpe: {sharpe} | PnL: {pnl} | Feedback: {}",
                    fb.reason()
                )))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use rara_domain::research::BacktestResult;

    use super::*;

    fn make_hypothesis(text: &str, parent: Option<Uuid>) -> Hypothesis {
        Hypothesis::builder()
            .text(text)
            .reason("test reason")
            .maybe_parent(parent)
            .build()
    }

    fn make_experiment(hypothesis_id: Uuid, code: &str, sharpe: Option<f64>) -> Experiment {
        let backtest = sharpe.map(|s| {
            BacktestResult::builder()
                .pnl(dec!(100.0))
                .sharpe_ratio(s)
                .max_drawdown(dec!(10.0))
                .win_rate(0.6)
                .trade_count(50)
                .build()
        });

        Experiment::builder()
            .hypothesis_id(hypothesis_id)
            .strategy_code(code)
            .maybe_backtest_result(backtest)
            .build()
    }

    fn make_feedback(experiment_id: Uuid, decision: bool, reason: &str) -> HypothesisFeedback {
        HypothesisFeedback::builder()
            .experiment_id(experiment_id)
            .decision(decision)
            .reason(reason)
            .observations("test obs")
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

    #[test]
    fn record_and_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let h = make_hypothesis("h1", None);
        trace.save_hypothesis(&h).unwrap();

        let exp0 = make_experiment(h.id(), "code0", Some(1.0));
        let fb0 = make_feedback(exp0.id(), true, "reason0");
        let idx0 = trace.record(&exp0, &fb0, &DagSelection::NewRoot).unwrap();
        assert_eq!(idx0, 0);

        let exp1 = make_experiment(h.id(), "code1", Some(1.5));
        let fb1 = make_feedback(exp1.id(), true, "reason1");
        let idx1 = trace.record(&exp1, &fb1, &DagSelection::Latest).unwrap();
        assert_eq!(idx1, 1);

        let exp2 = make_experiment(h.id(), "code2", Some(2.0));
        let fb2 = make_feedback(exp2.id(), false, "reason2");
        let idx2 = trace
            .record(&exp2, &fb2, &DagSelection::Specific(0))
            .unwrap();
        assert_eq!(idx2, 2);

        // Walk ancestors from idx1: should get idx1 -> idx0
        let anc = trace.ancestors(idx1).unwrap();
        assert_eq!(anc.len(), 2);
        assert_eq!(anc[0].0.id(), exp1.id());
        assert_eq!(anc[1].0.id(), exp0.id());

        // Walk ancestors from idx2: should get idx2 -> idx0
        let anc2 = trace.ancestors(idx2).unwrap();
        assert_eq!(anc2.len(), 2);
        assert_eq!(anc2[0].0.id(), exp2.id());
        assert_eq!(anc2[1].0.id(), exp0.id());
    }

    #[test]
    fn record_and_children() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let h = make_hypothesis("h1", None);
        trace.save_hypothesis(&h).unwrap();

        let exp0 = make_experiment(h.id(), "code0", Some(1.0));
        let fb0 = make_feedback(exp0.id(), true, "root reason");
        let idx0 = trace.record(&exp0, &fb0, &DagSelection::NewRoot).unwrap();

        let exp1 = make_experiment(h.id(), "code1", Some(1.5));
        let fb1 = make_feedback(exp1.id(), true, "child1 reason");
        trace
            .record(&exp1, &fb1, &DagSelection::Specific(idx0))
            .unwrap();

        let exp2 = make_experiment(h.id(), "code2", Some(0.5));
        let fb2 = make_feedback(exp2.id(), false, "child2 reason");
        trace
            .record(&exp2, &fb2, &DagSelection::Specific(idx0))
            .unwrap();

        let kids = trace.children(idx0).unwrap();
        assert_eq!(kids.len(), 2);

        let kid_ids: Vec<Uuid> = kids.iter().map(|(e, _)| e.id()).collect();
        assert!(kid_ids.contains(&exp1.id()));
        assert!(kid_ids.contains(&exp2.id()));
    }

    #[test]
    fn get_sota_picks_highest_sharpe() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let h = make_hypothesis("h1", None);
        trace.save_hypothesis(&h).unwrap();

        // Accepted with low Sharpe
        let exp_low = make_experiment(h.id(), "low", Some(0.5));
        let fb_low = make_feedback(exp_low.id(), true, "low sharpe reason");
        trace.record(&exp_low, &fb_low, &DagSelection::NewRoot).unwrap();

        // Rejected with high Sharpe (should not win)
        let exp_rej = make_experiment(h.id(), "rejected", Some(5.0));
        let fb_rej = make_feedback(exp_rej.id(), false, "rejected reason");
        trace.record(&exp_rej, &fb_rej, &DagSelection::Latest).unwrap();

        // Accepted with high Sharpe (should win)
        let exp_high = make_experiment(h.id(), "high", Some(2.5));
        let fb_high = make_feedback(exp_high.id(), true, "high sharpe reason");
        trace.record(&exp_high, &fb_high, &DagSelection::Latest).unwrap();

        let sota = trace.get_sota().unwrap().unwrap();
        assert_eq!(sota.0.id(), exp_high.id());
        assert!(sota.1.decision());
    }

    #[test]
    fn format_for_prompt_output() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        let h = make_hypothesis("mean reversion", None);
        trace.save_hypothesis(&h).unwrap();

        let exp0 = make_experiment(h.id(), "code0", Some(1.23));
        let fb0 = make_feedback(exp0.id(), true, "good performance");
        trace.record(&exp0, &fb0, &DagSelection::NewRoot).unwrap();

        let exp1 = make_experiment(h.id(), "code1", Some(-0.5));
        let fb1 = make_feedback(exp1.id(), false, "poor drawdown");
        trace.record(&exp1, &fb1, &DagSelection::Latest).unwrap();

        let output = trace.format_for_prompt(10).unwrap();
        assert!(output.contains("[Iteration 0]"));
        assert!(output.contains("[Iteration 1]"));
        assert!(output.contains("mean reversion"));
        assert!(output.contains("accepted"));
        assert!(output.contains("rejected"));
        assert!(output.contains("1.23"));
        assert!(output.contains("-0.50"));
        assert!(output.contains("good performance"));
        assert!(output.contains("poor drawdown"));

        // Test max_entries limit
        let limited = trace.format_for_prompt(1).unwrap();
        assert!(!limited.contains("[Iteration 0]"));
        assert!(limited.contains("[Iteration 1]"));
    }
}

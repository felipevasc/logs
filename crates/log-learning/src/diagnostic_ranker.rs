//! Bounded linear ranking with explicit incident outcomes. No line labels,
//! embeddings, probabilities in the product, or learning from user silence.
use crate::{Diagnosis, DiagnosticAssessment, DiagnosticSummary, DIAGNOSTIC_FEATURES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const RANKING_DIMENSIONS: usize = DIAGNOSTIC_FEATURES.len() + 2;
/// Count independent episode outcomes, not the many correlated hypothesis
/// pairs within one episode. Hoeffding's one-sided bound rejects weak gains.
pub(crate) fn consistent_ranking_gain(gains: &[i32]) -> bool {
    let wins = gains.iter().filter(|&&gain| gain > 0).count();
    let losses = gains.iter().filter(|&&gain| gain < 0).count();
    if gains.len() < 8 || wins < 6 || wins <= losses {
        return false;
    }
    let difference = (wins - losses) as f64;
    (-difference * difference / (2.0 * (wins + losses) as f64)).exp() <= 0.05
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRankingExample {
    pub episode: String,
    pub at: i64,
    pub diagnosis: Diagnosis,
    pub confirmed: bool,
    pub features: u64,
    pub base_score: u16,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticRanker {
    pub weights: BTreeMap<Diagnosis, Vec<f64>>,
    pub active: bool,
    pub training_episodes: usize,
    pub validation_episodes: usize,
    pub validation_pairs: usize,
    pub baseline_correct: usize,
    pub candidate_correct: usize,
}
impl DiagnosticRanker {
    pub fn validate(&self) -> Result<(), String> {
        if self.weights.len() > 14
            || self.weights.values().any(|w| {
                w.len() != RANKING_DIMENSIONS || w.iter().any(|v| !v.is_finite() || v.abs() > 4.0)
            })
            || self.training_episodes > 2048
            || self.validation_episodes > 2048
            || self.validation_pairs > 65536
            || self.baseline_correct > self.validation_pairs
            || self.candidate_correct > self.validation_pairs
            || self.active
                && (self.validation_pairs < 8 || self.candidate_correct <= self.baseline_correct)
        {
            return Err("Ranking diagnóstico inválido ou sem validação suficiente".into());
        }
        Ok(())
    }
    pub fn score(&self, diagnosis: Diagnosis, mask: u64, base: u16) -> f64 {
        let Some(weights) = self.weights.get(&diagnosis) else {
            return base as f64;
        };
        let x = features(mask, base);
        let dot = weights.iter().zip(x).map(|(w, x)| w * x).sum::<f64>();
        // The model can only reorder evidence-supported candidates, within a
        // small bound. It cannot generate a diagnosis or change its evidence.
        base as f64 + 8.0 * (dot / 4.0).tanh()
    }
    pub fn apply(&self, summary: &DiagnosticSummary, assessment: &mut DiagnosticAssessment) {
        if !self.active {
            return;
        }
        let mask = summary.mask();
        assessment.hypotheses.sort_by(|a, b| {
            self.score(b.diagnosis, mask, b.score)
                .total_cmp(&self.score(a.diagnosis, mask, a.score))
                .then(a.diagnosis.cmp(&b.diagnosis))
        });
    }
}
fn features(mask: u64, base: u16) -> [f64; RANKING_DIMENSIONS] {
    let mut x = [0.0; RANKING_DIMENSIONS];
    x[0] = 1.0;
    for (i, f) in DIAGNOSTIC_FEATURES.iter().enumerate() {
        x[i + 1] = f64::from(mask & f.bit() != 0);
    }
    x[RANKING_DIMENSIONS - 1] = base as f64 / 85.0;
    x
}
pub(crate) fn fit(examples: &[&DiagnosticRankingExample]) -> DiagnosticRanker {
    let mut model = DiagnosticRanker::default();
    for _ in 0..8 {
        for e in examples {
            let weights = model
                .weights
                .entry(e.diagnosis)
                .or_insert_with(|| vec![0.0; RANKING_DIMENSIONS]);
            let x = features(e.features, e.base_score);
            let dot = weights.iter().zip(x).map(|(w, x)| w * x).sum::<f64>();
            let probability = 1.0 / (1.0 + (-dot.clamp(-20.0, 20.0)).exp());
            let residual = f64::from(e.confirmed) - probability;
            for (w, x) in weights.iter_mut().zip(x) {
                *w = (*w + 0.025 * (residual * x - 0.02 * *w)).clamp(-4.0, 4.0);
            }
        }
    }
    model
}
pub fn train_diagnostic_ranker(
    examples: &[DiagnosticRankingExample],
) -> Result<DiagnosticRanker, String> {
    if examples.len() > 2048
        || examples.iter().any(|e| {
            e.episode.len() > 128 || e.base_score > 85 || !crate::valid_feature_mask(e.features)
        })
    {
        return Err("Exemplos de ranking fora do orçamento".into());
    }
    let mut episodes = BTreeMap::<String, i64>::new();
    let mut unique = BTreeSet::new();
    for e in examples {
        if !unique.insert((&e.episode, e.diagnosis)) {
            return Err("Rótulo repetido para o mesmo episódio e hipótese".into());
        }
        episodes
            .entry(e.episode.clone())
            .and_modify(|at| *at = (*at).min(e.at))
            .or_insert(e.at);
    }
    let mut episodes = episodes.into_iter().collect::<Vec<_>>();
    episodes.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    if episodes.len() < 32 {
        return Ok(DiagnosticRanker {
            training_episodes: episodes.len(),
            ..Default::default()
        });
    }
    let split = episodes.len() * 3 / 4;
    let training = episodes[..split]
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<BTreeSet<_>>();
    let mut model = fit(&examples
        .iter()
        .filter(|e| training.contains(e.episode.as_str()))
        .collect::<Vec<_>>());
    model.training_episodes = training.len();
    model.validation_episodes = episodes.len() - split;
    let mut by_family = BTreeMap::<&str, (usize, usize)>::new();
    let mut episode_gains = Vec::new();
    for (id, _) in &episodes[split..] {
        let group = examples
            .iter()
            .filter(|e| e.episode == *id)
            .collect::<Vec<_>>();
        let mut paired = false;
        let mut gain = 0;
        for positive in group.iter().filter(|e| e.confirmed) {
            for negative in group.iter().filter(|e| !e.confirmed) {
                if positive.features != negative.features {
                    continue;
                }
                let baseline = positive.base_score > negative.base_score;
                let candidate =
                    model.score(positive.diagnosis, positive.features, positive.base_score)
                        > model.score(negative.diagnosis, negative.features, negative.base_score);
                paired = true;
                gain += i32::from(candidate) - i32::from(baseline);
                model.validation_pairs += 1;
                model.baseline_correct += usize::from(baseline);
                model.candidate_correct += usize::from(candidate);
                let entry = by_family.entry(positive.diagnosis.family()).or_default();
                entry.0 += usize::from(baseline);
                entry.1 += usize::from(candidate);
            }
        }
        if paired {
            episode_gains.push(gain);
        }
    }
    model.active = model.validation_pairs >= 8
        && consistent_ranking_gain(&episode_gains)
        && model.candidate_correct > model.baseline_correct
        && by_family.values().all(|(a, b)| b >= a);
    model.validate()?;
    Ok(model)
}

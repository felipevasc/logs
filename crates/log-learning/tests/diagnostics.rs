use log_learning::*;
use std::collections::BTreeMap;
fn assess(record: DiagnosticRecord) -> (DiagnosticEventFacts, DiagnosticAssessment) {
    let facts = extract_diagnostic(&record);
    facts.validate().unwrap();
    let mut summary = DiagnosticSummary::default();
    summary.observe(
        DiagnosticReference {
            event_ref: "original:1".into(),
            event_id: 1,
            timestamp: Some(1),
        },
        facts.mask,
        facts.typed,
        facts.failure,
    );
    let result = assess_diagnostic(&summary, &diagnostic_rules());
    (facts, result)
}
fn failed(message: &str) -> DiagnosticRecord {
    DiagnosticRecord {
        message: message.into(),
        failure: true,
        ..Default::default()
    }
}
#[test]
fn pool_reports_do_not_invent_occupancy_and_known_measurements_strengthen_support() {
    let (facts, reported) = assess(failed(
        "HikariPool-1 - Connection is not available, request timed out after 30000ms",
    ));
    assert_eq!(reported.hypotheses[0].diagnosis, Diagnosis::PoolExhaustion);
    assert_eq!(facts.mask & DiagnosticFeature::PoolAtLimit.bit(), 0);
    assert_eq!(reported.hypotheses[0].support[0].typed_observations, 0);
    assert_eq!(
        reported.hypotheses[0].support[0].event_refs[0].event_ref,
        "original:1"
    );
    let mut record = failed("timeout acquiring connection");
    record.metrics = BTreeMap::from([
        (DiagnosticMetric::PoolActive, 40.0),
        (DiagnosticMetric::PoolLimit, 40.0),
        (DiagnosticMetric::PoolPending, 7.0),
        (DiagnosticMetric::AcquireMs, 30000.0),
    ]);
    let (_, measured) = assess(record);
    assert!(measured.hypotheses[0].score > reported.hypotheses[0].score);
    assert_eq!(measured.measured_accuracy, None);
}
#[test]
fn failures_are_distinguished_by_phase_and_protocol() {
    let (_,response)=assess(failed("upstream timed out (110: Connection timed out) while reading response header from upstream"));
    assert_eq!(response.hypotheses[0].diagnosis, Diagnosis::DependencySlow);
    assert!(!response
        .hypotheses
        .iter()
        .any(|h| h.diagnosis == Diagnosis::DependencyUnavailable));
    let (_, connection) = assess(failed("connect timed out while connecting to upstream"));
    assert_eq!(
        connection.hypotheses[0].diagnosis,
        Diagnosis::DependencyUnavailable
    );
    let (_, windows) = assess(DiagnosticRecord {
        code: "53300".into(),
        product: "windows-security".into(),
        failure: true,
        ..Default::default()
    });
    assert!(windows.abstention.is_some());
    let (_, postgres) = assess(DiagnosticRecord {
        code: "53300".into(),
        product: "postgresql".into(),
        failure: true,
        ..Default::default()
    });
    assert_eq!(
        postgres.hypotheses[0].diagnosis,
        Diagnosis::DatabaseConnectionLimit
    );
}
#[test]
fn benign_quotes_negation_and_success_never_become_diagnoses() {
    for message in [
        "example: connection pool exhausted",
        "request.body=connection refused",
        "certificate is not expired",
        "no deadlock detected",
        "no read timeout",
    ] {
        let (_, assessment) = assess(failed(message));
        assert!(assessment.hypotheses.is_empty(), "{message}");
    }
    let (_, success) = assess(DiagnosticRecord {
        message: "connection refused".into(),
        success: true,
        failure: true,
        ..Default::default()
    });
    assert!(success.abstention.is_some());
    let (_, docs) = assess(DiagnosticRecord {
        documentation: true,
        ..failed("connection refused")
    });
    assert!(docs.abstention.is_some());
}
#[test]
fn absent_metrics_are_unknown_and_ordinary_pool_utilization_is_not_an_incident() {
    let (_, healthy) = assess(DiagnosticRecord {
        metrics: BTreeMap::from([
            (DiagnosticMetric::PoolActive, 50.0),
            (DiagnosticMetric::PoolLimit, 50.0),
        ]),
        ..Default::default()
    });
    assert!(healthy.hypotheses.is_empty());
    let (facts, _) = assess(DiagnosticRecord {
        metrics: BTreeMap::from([(DiagnosticMetric::QueryMs, 8000.0)]),
        ..Default::default()
    });
    assert_eq!(facts.mask & DiagnosticFeature::QuerySlow.bit(), 0);
    let (_, slow) = assess(DiagnosticRecord {
        success: true,
        metrics: BTreeMap::from([
            (DiagnosticMetric::QueryMs, 8000.0),
            (DiagnosticMetric::QueryLimitMs, 1000.0),
        ]),
        ..Default::default()
    });
    assert_eq!(slow.hypotheses[0].diagnosis, Diagnosis::SlowDatabase);
}
#[test]
fn auth_reasons_are_not_interchangeable_and_generic_failure_abstains() {
    let (_, expired) = assess(DiagnosticRecord {
        product: "authentication".into(),
        auth_reason: "token_expired".into(),
        ..failed("authentication failed")
    });
    assert_eq!(
        expired.hypotheses[0].diagnosis,
        Diagnosis::ExpiredCredentials
    );
    let (_, unknown) = assess(DiagnosticRecord {
        product: "authentication".into(),
        ..failed("authentication failed")
    });
    assert!(unknown.abstention.is_some());
    assert_eq!(unknown.next_queries, [DiagnosticQuery::Overview]);
}
#[test]
fn repetition_and_dependent_reports_cannot_inflate_confidence() {
    let facts = extract_diagnostic(&failed(
        "timeout acquiring connection; connection pool exhausted",
    ));
    let mut summary = DiagnosticSummary::default();
    for i in 0..1000 {
        summary.observe(
            DiagnosticReference {
                event_ref: format!("ref:{i}"),
                event_id: i,
                timestamp: Some(i),
            },
            facts.mask,
            facts.typed,
            true,
        );
    }
    let assessment = assess_diagnostic(&summary, &diagnostic_rules());
    assert_eq!(assessment.hypotheses[0].score, 40);
    assert!(assessment.hypotheses[0]
        .support
        .iter()
        .all(|s| s.event_refs.len() <= 8));
    assert!(assessment.hypotheses.len() <= 3);
    for rule in diagnostic_rules() {
        rule.validate().unwrap();
    }
}
fn labelled(flipped_holdout: bool) -> Vec<DiagnosticRankingExample> {
    let mask = DiagnosticFeature::AcquireTimeout.bit() | DiagnosticFeature::DnsFailure.bit();
    (0..32)
        .flat_map(|i| {
            let positive = if flipped_holdout && i >= 24 {
                Diagnosis::DnsResolution
            } else {
                Diagnosis::PoolExhaustion
            };
            [Diagnosis::PoolExhaustion, Diagnosis::DnsResolution]
                .into_iter()
                .map(move |diagnosis| DiagnosticRankingExample {
                    episode: format!("incident:{i}"),
                    at: i,
                    diagnosis,
                    confirmed: diagnosis == positive,
                    features: mask,
                    base_score: 40,
                })
        })
        .collect()
}
#[test]
fn ranking_requires_independent_later_resolutions_and_rejects_regression() {
    assert_eq!(DIAGNOSTIC_FEATURES.len() + 2, RANKING_DIMENSIONS);
    let examples = labelled(false);
    assert!(!train_diagnostic_ranker(&examples[..20]).unwrap().active);
    let accepted = train_diagnostic_ranker(&examples).unwrap();
    assert!(accepted.active);
    assert_eq!(accepted.validation_episodes, 8);
    assert_eq!(accepted.validation_pairs, 8);
    assert!(accepted.candidate_correct > accepted.baseline_correct);
    assert!(!train_diagnostic_ranker(&labelled(true)).unwrap().active);
    let mut isolated_gain = examples.clone();
    for example in isolated_gain.iter_mut().filter(|e| e.at >= 25) {
        example.base_score = if example.confirmed { 60 } else { 20 };
    }
    let weak = train_diagnostic_ranker(&isolated_gain).unwrap();
    assert!(weak.candidate_correct > weak.baseline_correct);
    assert!(
        !weak.active,
        "one improved incident cannot promote the ranker"
    );
    assert!(serde_json::to_vec(&accepted).unwrap().len() < 20_000);
    let mut duplicate = examples.clone();
    duplicate.push(examples[0].clone());
    assert!(train_diagnostic_ranker(&duplicate).is_err());
}
#[test]
fn shared_knowledge_is_reproducible_rejects_private_fields_and_rechecks_report() {
    let baseline = DiagnosticKnowledge::baseline().unwrap();
    baseline.validate().unwrap();
    assert_eq!(baseline.evaluation.counts.cases, 50);
    assert_eq!(baseline.evaluation.counts.false_diagnoses, 0);
    assert_eq!(
        baseline.evaluation.counts.top_three,
        baseline.evaluation.counts.eligible
    );
    assert!(!baseline.evaluation.representative);
    let mut contribution = DiagnosticContribution {
        schema_version: 1,
        id: String::new(),
        origin: "a".repeat(32),
        episode: "b".repeat(32),
        revision: 1,
        features: DiagnosticFeature::AcquireTimeout.bit(),
        scores: BTreeMap::from([(Diagnosis::PoolExhaustion, 35)]),
        labels: vec![DiagnosticLabel {
            diagnosis: Diagnosis::PoolExhaustion,
            confirmed: true,
        }],
    };
    contribution.seal();
    contribution.validate().unwrap();
    let mut updated = contribution.clone();
    updated.revision = 2;
    updated.labels[0].confirmed = false;
    updated.seal();
    let a = DiagnosticKnowledge::consolidate(vec![contribution.clone(), updated.clone()]).unwrap();
    let b = DiagnosticKnowledge::consolidate(vec![updated.clone(), contribution.clone(), updated])
        .unwrap();
    assert_eq!(a.revision(), b.revision());
    assert_eq!(a.episodes.len(), 1);
    assert!(!a.ranker.active);
    let mut retracted = contribution.clone();
    retracted.revision = 3;
    retracted.features = 0;
    retracted.scores.clear();
    retracted.labels.clear();
    retracted.seal();
    let removed =
        DiagnosticKnowledge::consolidate(vec![retracted.clone(), contribution.clone()]).unwrap();
    assert!(removed.episodes.values().next().unwrap().labels.is_empty());
    assert!(!removed.ranker.active);
    retracted.features = DiagnosticFeature::AcquireTimeout.bit();
    retracted.seal();
    assert!(
        retracted.validate().is_err(),
        "retractions cannot carry extra observations"
    );
    let mut wire = serde_json::to_value(&contribution).unwrap();
    wire["message"] = serde_json::json!("secret log");
    assert!(serde_json::from_value::<DiagnosticContribution>(wire).is_err());
    let mut forged = baseline;
    forged.evaluation.counts.top_three = 0;
    assert!(forged.validate().is_err());
    let mut corpus = diagnostic_evaluation_corpus().unwrap();
    let mut leaked = corpus[0].clone();
    leaked.id = "different-id".into();
    leaked.split = "development".into();
    corpus.push(leaked);
    assert!(
        evaluate_diagnostics(&diagnostic_rules(), &DiagnosticRanker::default(), &corpus).is_err()
    );
}
#[test]
fn shared_training_reserves_origins_and_caps_large_contributors() {
    let mut contributions = Vec::new();
    for origin in 0..4 {
        for episode in 0..12 {
            let mut c = DiagnosticContribution {
                schema_version: 1,
                id: String::new(),
                origin: format!("{origin:032x}"),
                episode: format!("{episode:032x}"),
                revision: episode + 1,
                features: DiagnosticFeature::AcquireTimeout.bit()
                    | DiagnosticFeature::DnsFailure.bit(),
                scores: BTreeMap::from([
                    (Diagnosis::PoolExhaustion, 40),
                    (Diagnosis::DnsResolution, 40),
                ]),
                labels: vec![
                    DiagnosticLabel {
                        diagnosis: Diagnosis::PoolExhaustion,
                        confirmed: true,
                    },
                    DiagnosticLabel {
                        diagnosis: Diagnosis::DnsResolution,
                        confirmed: false,
                    },
                ],
            };
            c.seal();
            contributions.push(c);
        }
    }
    let knowledge = DiagnosticKnowledge::consolidate(contributions).unwrap();
    assert!(knowledge.ranker.active);
    assert_eq!(knowledge.ranker.validation_episodes, 12);
    assert_eq!(knowledge.ranker.training_episodes, 36);
    let model_bytes = serde_json::to_vec(&knowledge.ranker).unwrap().len();
    assert!(model_bytes < 20_000);
    knowledge.validate().unwrap();
    let prototype = knowledge.episodes.values().next().unwrap();
    let mut grown = Vec::new();
    for origin in 0..32 {
        for episode in 0..80 {
            let mut contribution = prototype.clone();
            contribution.origin = format!("{origin:032x}");
            contribution.episode = format!("{episode:032x}");
            contribution.revision = episode + 1;
            contribution.seal();
            grown.push(contribution);
        }
    }
    let grown = DiagnosticKnowledge::consolidate(grown).unwrap();
    assert_eq!(grown.episodes.len(), 2560);
    assert!(
        grown.ranker.active,
        "a larger history must remain trainable within the fixed budget"
    );
    assert!(grown.ranker.training_episodes * 2 <= 2048);
    assert!(grown.ranker.validation_episodes * 2 <= 2048);
}

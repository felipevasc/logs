use log_learning::*;

fn key() -> String {
    digest(b"service-a")
}
#[test]
fn semantic_codes_and_negations_survive_variable_token_learning() {
    let mut model = Model::default();
    for _ in 0..120 {
        model.train(
            &key(),
            &event("database reported error SQLSTATE=23505"),
            None,
        );
    }
    assert!(model
        .score(
            &key(),
            &event("database reported error SQLSTATE=08001"),
            None
        )
        .unusual());
    let mut model = Model::default();
    for _ in 0..120 {
        model.train(&key(), &event("request completed with no error"), None);
    }
    assert!(model
        .score(&key(), &event("request completed with an error"), None)
        .unusual());
    assert_ne!(tokens("response status 200"), tokens("response status 503"));
    assert_eq!(
        tokens("request for user123 completed"),
        tokens("request for user456 completed")
    );
}
#[test]
fn shared_domains_cover_the_native_normalizers_without_guessing_generic_products() {
    assert_eq!(Domain::from_product("web"), Domain::Web);
    assert_eq!(
        Domain::from_product("windows-security"),
        Domain::OperatingSystem
    );
    assert_eq!(Domain::from_product("sshd"), Domain::Authentication);
    for product in ["aws", "gcp", "azure", "kubernetes", "github"] {
        assert_eq!(Domain::from_product(product), Domain::Cloud);
    }
    assert_eq!(Domain::from_product("generic"), Domain::Other);
}

#[test]
fn compacted_receipts_never_retrain_old_observations_and_use_fixed_memory() {
    let mut seen = SeenFilter::default();
    for i in 0..20000 {
        seen.insert(&digest(format!("receipt-{i}").as_bytes()))
            .unwrap();
    }
    for i in 0..20000 {
        assert!(seen.contains(&digest(format!("receipt-{i}").as_bytes())));
    }
    assert_eq!(seen.bytes().len(), 2 * 1024 * 1024);
    assert!(seen.estimated_false_positive_rate() < 0.00001);
    let restored = SeenFilter::from_bytes(seen.bytes().to_vec()).unwrap();
    assert!(restored.contains(&digest(b"receipt-10")));
    assert!(!restored.contains(&digest(b"new-receipt")));
}

#[test]
fn cpu_budget_smoke_keeps_inference_model_size_constant() {
    let model = trained();
    let bytes = serde_json::to_vec(&model).unwrap().len();
    let started = std::time::Instant::now();
    let mut alerts = 0;
    for i in 0..100000 {
        let mut e = event("request completed successfully");
        if i % 1000 == 0 {
            e.latency_ms = Some(8000.0);
        }
        alerts += usize::from(model.score(&key(), &e, None).unusual());
    }
    assert_eq!(alerts, 100);
    assert_eq!(serde_json::to_vec(&model).unwrap().len(), bytes);
    assert!(bytes < 20000);
    eprintln!(
        "learning workload: 100000 observations, {} ms, {bytes} model bytes",
        started.elapsed().as_millis()
    );
}

#[test]
fn persistent_moderate_shifts_accumulate_without_flagging_a_stable_reference() {
    let mut histogram = Histogram::default();
    for _ in 0..200 {
        histogram.push(Some(12.0));
    }
    let mut stable = DurationChange::new(&histogram).unwrap();
    for _ in 0..100 {
        assert!(!stable.observe(12.0, 30));
    }
    let mut shifted = DurationChange::new(&histogram).unwrap();
    assert!((0..10).any(|_| shifted.observe(20.0, 30)));
    shifted.reset();
    assert!(!shifted.observe(12.0, 30));
}
fn event(message: &str) -> Observation<'_> {
    Observation {
        message,
        failure: false,
        latency_ms: Some(12.0),
        bytes: Some(100.0),
    }
}
fn trained() -> Model {
    let mut model = Model::default();
    for _ in 0..120 {
        model.train(&key(), &event("request completed successfully"), None);
    }
    model
}
#[test]
fn learns_variable_message_families_without_labels_and_keeps_distinct_messages() {
    let mut model = Model::default();
    for i in 0..80 {
        model.train(
            &key(),
            &event(&format!("request for account user{i} completed")),
            None,
        );
    }
    assert!(!model
        .score(
            &key(),
            &event("request for account another-user completed"),
            None
        )
        .unusual());
    assert!(model
        .score(&key(), &event("database connection pool exhausted"), None)
        .unusual());
    model.validate().unwrap();
}
#[test]
fn numeric_and_joint_detectors_keep_units_missing_values_and_contexts_independent() {
    let model = trained();
    let mut slow = event("request completed successfully");
    slow.latency_ms = Some(10000.0);
    assert!(model
        .score(&key(), &slow, None)
        .deviations
        .iter()
        .any(|d| d.kind == "latency_tail"));
    slow.latency_ms = None;
    assert!(!model
        .score(&key(), &slow, None)
        .deviations
        .iter()
        .any(|d| d.kind == "latency_tail"));
    slow.latency_ms = Some(f64::NAN);
    assert!(!model
        .score(&key(), &slow, None)
        .deviations
        .iter()
        .any(|d| d.kind == "latency_tail"));
    assert!(!model.score(&digest(b"other-tenant"), &slow, None).ready);
    let mut joint = Model::default();
    for i in 0..200 {
        let mut e = event("request completed successfully");
        e.latency_ms = Some(if i % 2 == 0 { 8.0 } else { 100.0 });
        e.bytes = Some(if i % 2 == 0 { 8000.0 } else { 100.0 });
        joint.train(&key(), &e, None);
    }
    let mut e = event("request completed successfully");
    e.latency_ms = Some(100.0);
    e.bytes = Some(8000.0);
    assert!(joint
        .score(&key(), &e, None)
        .deviations
        .iter()
        .any(|d| d.kind == "joint_novelty"));
}
#[test]
fn learns_sequence_probabilities_without_inventing_a_link() {
    let mut model = Model::default();
    let mut start = None;
    for _ in 0..100 {
        start = model.train(&key(), &event("operation started"), None);
        model.train(&key(), &event("operation completed"), start);
        model.train(&key(), &event("operation cancelled"), None);
    }
    assert!(model
        .score(&key(), &event("operation cancelled"), start)
        .deviations
        .iter()
        .any(|d| d.kind == "sequence_surprise"));
    assert!(!model
        .score(&key(), &event("operation cancelled"), None)
        .deviations
        .iter()
        .any(|d| d.kind == "sequence_surprise"));
}
#[test]
fn budgets_are_explicit_and_saturation_never_invents_novelty() {
    let mut model = Model::default();
    model.limits.max_contexts = 1;
    model.limits.max_templates = 8;
    for i in 0..80 {
        model.train(
            &key(),
            &event(&format!(
                "{} {}",
                ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'][i % 10],
                ['x', 'y', 'z', 'w', 'q', 'r', 's', 't'][i % 8]
            )),
            None,
        );
    }
    assert!(model.contexts[&key()].overflow > 0);
    assert_eq!(model.contexts[&key()].clusters.len(), 8);
    assert!(model
        .train(&digest(b"extra"), &event("another message"), None)
        .is_none());
    assert!(!model
        .score(&key(), &event("brand new message"), None)
        .deviations
        .iter()
        .any(|d| d.kind == "message_novelty"));
    model.validate().unwrap();
}
#[test]
fn shared_merges_are_idempotent_order_independent_and_separated_by_domain() {
    let a = PublicState::new("read", "success", Some(10.0), Some(100.0), 4);
    let b = PublicState::new("write", "failure", Some(5000.0), None, 5);
    let mut c = Contribution::new(Domain::Application);
    for _ in 0..20 {
        c.observe(a, None);
        c.observe(b, Some(a));
    }
    c.seal();
    c.validate().unwrap();
    let mut d = Contribution::new(Domain::Database);
    d.observe(a, None);
    d.seal();
    let mut x = SharedModel::default();
    assert!(x.merge(&c).unwrap());
    assert!(!x.merge(&c).unwrap());
    x.merge(&d).unwrap();
    let mut y = SharedModel::default();
    y.merge(&d).unwrap();
    y.merge(&c).unwrap();
    assert_eq!(x.revision(), y.revision());
    assert_eq!(x.support(Domain::Application, a), (20, 40));
    assert_eq!(x.support(Domain::Database, a), (1, 1));
}
#[test]
fn public_protocol_rejects_private_fields_tampering_and_impossible_counts() {
    let mut c = Contribution::new(Domain::Other);
    let state = PublicState::new("secret-project", "secret-user", None, None, 3);
    c.observe(state, None);
    c.seal();
    let mut json = serde_json::to_value(&c).unwrap();
    json["message"] = serde_json::json!("private password=secret");
    assert!(serde_json::from_value::<Contribution>(json).is_err());
    let text = serde_json::to_string(&c).unwrap();
    assert!(!text.contains("secret"));
    c.states[0].count += 1;
    assert!(c.validate().is_err());
    c.seal();
    c.transitions.push(TransitionCount {
        from: state,
        to: state,
        count: 100,
    });
    c.seal();
    assert!(c.validate().is_err());
    let invalid =
        serde_json::json!({"schema_version":1,"contributions":[],"domains":{"private-name":{}}});
    assert!(serde_json::from_value::<SharedModel>(invalid).is_err());
}
#[test]
fn small_cohorts_remain_uncertain() {
    let (_, small) = rate_interval(0, 3);
    let (large, _) = rate_interval(50, 50);
    assert!(small > 0.5);
    assert!(large > 0.9);
}
#[test]
fn serialized_models_round_trip_and_invalid_versions_are_rejected() {
    let model = trained();
    let restored: Model = serde_json::from_slice(&serde_json::to_vec(&model).unwrap()).unwrap();
    restored.validate().unwrap();
    assert_eq!(
        model
            .score(&key(), &event("new unknown failure message"), None)
            .unusual(),
        restored
            .score(&key(), &event("new unknown failure message"), None)
            .unusual()
    );
    let mut bad = restored;
    bad.schema_version = 100;
    assert!(bad.validate().is_err());
}

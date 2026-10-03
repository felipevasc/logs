//! Portable Case-owned security definitions. JSON rule bodies are retained as
//! bounded strings so native exact-number codecs never reinterpret provenance.
use crate::analysis_context::Diagnostic;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};
const MAX_BYTES: usize = 2 << 20;
pub(crate) const MAX_SIGMA_FILES: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigmaSource {
    pub name: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_detection_settings")]
    pub detection_settings_json: String,
    #[serde(default)]
    pub custom_rules_json: Option<String>,
    #[serde(default)]
    pub sigma_sources: Vec<SigmaSource>,
    #[serde(default)]
    pub threat_catalog_json: Option<String>,
}
fn default_detection_settings() -> String {
    serde_json::to_string(&crate::detections::Settings::default()).expect("default settings")
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            detection_settings_json: default_detection_settings(),
            custom_rules_json: None,
            sigma_sources: vec![],
            threat_catalog_json: None,
        }
    }
}
impl Settings {
    pub(crate) fn detection_settings(&self) -> Result<crate::detections::Settings, String> {
        serde_json::from_str(&self.detection_settings_json)
            .map_err(|e| format!("Ajustes de detecção inválidos: {e}"))
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_BYTES {
            return Err("As definições de segurança do Caso excedem 2 MiB.".into());
        }
        let settings = self.detection_settings()?;
        crate::detections::validate_settings(&settings, &settings)?;
        if self.sigma_sources.len() > MAX_SIGMA_FILES
            || self.sigma_sources.iter().any(|s| {
                s.name.is_empty()
                    || s.name.len() > 255
                    || s.name.contains(['/', '\\'])
                    || s.text.len() > MAX_BYTES
            })
        {
            return Err("Fontes Sigma inválidas ou acima do limite de 512 arquivos.".into());
        }
        // Compile with the same validation and regex budgets as analysis, before CAS.
        if self.custom_rules_json.is_some() || !self.sigma_sources.is_empty() {
            crate::detections::ruleset_for(self)?;
        }
        if let Some(text) = &self.threat_catalog_json {
            crate::threats::compile_snapshot(text)?;
        }
        Ok(())
    }
    pub(crate) fn signature(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("security snapshot"))
        )
    }
}
/// Only immutable captured interpretation (or immutable shipped defaults) is
/// consulted. No implicit read of the destination profile is permitted.
thread_local! { static COMPILING: std::cell::RefCell<Option<std::sync::Arc<Settings>>> = const { std::cell::RefCell::new(None) }; }
pub(crate) fn compiling<T>(snapshot: &Settings, f: impl FnOnce() -> T) -> T {
    with_compiling(Some(std::sync::Arc::new(snapshot.clone())), f)
}
pub(crate) fn captured_compiling() -> Option<std::sync::Arc<Settings>> { COMPILING.with(|slot| slot.borrow().clone()) }
pub(crate) fn with_compiling<T>(snapshot: Option<std::sync::Arc<Settings>>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<std::sync::Arc<Settings>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            COMPILING.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore =
        Restore(COMPILING.with(|slot| slot.replace(snapshot)));
    f()
}
pub(crate) fn with<T>(f: impl FnOnce(&Settings) -> T) -> T {
    if let Some(snapshot) = COMPILING.with(|slot| slot.borrow().clone()) {
        return f(&snapshot);
    }
    if let Some(admitted) = crate::analysis_runtime::current() {
        return f(&admitted.interpretation.security);
    }
    static DEFAULT: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();
    f(DEFAULT.get_or_init(Settings::default))
}
pub(crate) fn read_bounded(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err("Arquivo de segurança excede 2 MiB; original preservado.".into());
    }
    Ok(text)
}
pub(crate) fn local_legacy(dir: &Path) -> (Settings, Vec<Diagnostic>) {
    let mut settings = Settings::default();
    let mut diagnostics = vec![];
    let mut import = |name: &str, change: &dyn Fn(&mut Settings, String)| {
        let path = dir.join(name);
        if !path.exists() {
            return;
        }
        let result = read_bounded(&path).and_then(|text| {
            let mut candidate = settings.clone();
            change(&mut candidate, text);
            candidate.validate()?;
            settings = candidate;
            Ok(())
        });
        if let Err(error) = result {
            diagnostics.push(diagnostic(name, &error));
        }
    };
    import("detections.json", &|s, text| {
        s.detection_settings_json = text
    });
    // Detection conditions can reference threat IDs, so migrate the catalog first.
    import("threat-rules.json", &|s, text| {
        s.threat_catalog_json = Some(text)
    });
    import("detection-rules.json", &|s, text| {
        s.custom_rules_json = Some(text)
    });
    let sigma = (|| -> Result<(), String> {
        let files = crate::sigma::rule_files(&dir.join("sigma"));
        if files.len() > MAX_SIGMA_FILES {
            return Err("Mais de 512 arquivos Sigma.".into());
        }
        let mut candidate = settings.clone();
        let mut bytes = 0usize;
        for file in files {
            crate::operations::check()?;
            let text = read_bounded(&file)?;
            bytes = bytes.saturating_add(text.len());
            if bytes > MAX_BYTES {
                return Err("Sigma excede 2 MiB.".into());
            }
            candidate.sigma_sources.push(SigmaSource {
                name: file
                    .file_name()
                    .ok_or("Arquivo Sigma sem nome")?
                    .to_string_lossy()
                    .into_owned(),
                text,
            });
        }
        candidate.validate()?;
        settings = candidate;
        Ok(())
    })();
    if let Err(error) = sigma {
        diagnostics.push(diagnostic("sigma", &error));
    }
    (settings, diagnostics)
}
fn diagnostic(name: &str, error: &str) -> Diagnostic {
    Diagnostic { definition_index: None, code: format!("legacy_interpretation_unavailable_{}", match name { "detections.json" => "security_settings", "detection-rules.json" => "security_rules", "threat-rules.json" => "security_threats", _ => "security_sigma" }), message: format!("As definições legadas de segurança {name} não foram ativadas ({error}). Os originais foram preservados; revise a segurança deste Caso.") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_ignore_local_overrides_and_definitions_remain_exact_strings() {
        let defaults = Settings::default();
        defaults.validate().unwrap();
        assert!(
            defaults.custom_rules_json.is_none()
                && defaults.sigma_sources.is_empty()
                && defaults.threat_catalog_json.is_none()
        );
        let text = r#"{"exact":9007199254740993,"decimal":0.1000000000000000001}"#;
        let mut source = defaults.clone();
        source.custom_rules_json = Some(text.into());
        let copy: Settings =
            serde_json::from_str(&serde_json::to_string(&source).unwrap()).unwrap();
        assert_eq!(copy.custom_rules_json.as_deref(), Some(text));
        assert_ne!(source.signature(), defaults.signature());
    }
    #[test]
    fn migration_copies_independent_settings_and_preserves_invalid_originals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("detections.json"),
            r#"{"disabled":["case-a"],"threats":false}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("threat-rules.json"), "invalid").unwrap();
        let (a, diagnostics) = local_legacy(dir.path());
        let mut b = a.clone();
        b.detection_settings_json = default_detection_settings();
        assert_eq!(a.detection_settings().unwrap().disabled, vec!["case-a"]);
        assert!(b.detection_settings().unwrap().disabled.is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("threat-rules.json")).unwrap(),
            "invalid"
        );
    }
    fn custom(pattern: &str) -> Settings {
        let mut security = Settings::default();
        security.threat_catalog_json = Some(serde_json::json!({"version":1,"name":"Case catalog","rules":[{"id":"case.signal","name":"Signal","category":"test","severity":"medium","kind":"indicator","pattern":pattern,"description":"Case fixture","enabled":true}]}).to_string());
        security.custom_rules_json = Some(serde_json::json!({"version":1,"rules":[{"id":"case.rule","name":"Case rule","severity":"medium","kind":"single","where":"regra:case.signal"}]}).to_string());
        security
    }
    #[test]
    fn immutable_compilation_is_content_scoped_even_when_rule_ids_are_equal() {
        let a = custom("CASE_ALPHA"); let mut b = custom("CASE_BRAVO");
        a.validate().unwrap(); b.validate().unwrap();
        let first = crate::detections::ruleset_for(&a).unwrap();
        let second = crate::detections::ruleset_for(&b).unwrap();
        let mut event = crate::model::Event::empty(); event.message = "CASE_ALPHA".into();
        assert!(first.find("case.rule").unwrap().matches(&event));
        assert!(!second.find("case.rule").unwrap().matches(&event));
        assert_eq!(first.find("case.rule").unwrap().origin, "case");
        b.detection_settings_json = r#"{"disabled":["case.rule"]}"#.into();
        assert!(!crate::detections::ruleset_for(&b).unwrap().find("case.rule").unwrap().enabled);
        assert!(first.find("case.rule").unwrap().enabled);
        let builtins = crate::detections::ruleset_for(&Settings::default()).unwrap();
        assert!(builtins.find("case.rule").is_none());
        assert!(first.find("case.rule").unwrap().matches(&event));
    }
    #[test]
    fn malformed_and_oversized_custom_definitions_are_rejected_without_publication() {
        let mut settings = Settings::default(); settings.custom_rules_json = Some("invalid".into()); assert!(settings.validate().is_err());
        settings.custom_rules_json = None; settings.sigma_sources = vec![SigmaSource { name: "../escape.yml".into(), text: "title: test".into() }]; assert!(settings.validate().is_err());
        settings.sigma_sources.clear(); settings.detection_settings_json = "x".repeat(MAX_BYTES + 1); assert!(settings.validate().is_err());
    }

    #[test]
    fn custom_filter_validation_and_prepared_jobs_use_the_captured_security_snapshot() {
        let a = custom("CASE_ALPHA"); let b = custom("CASE_BRAVO");
        let filters = vec![
            crate::query::Filter { column: "_all".into(), op: "detection".into(), value: "case.rule".into(), value2: None },
            crate::query::Filter { column: "_all".into(), op: "query".into(), value: "regra:case.signal".into(), value2: None },
        ];
        assert!(crate::workspace::validate(&filters).is_err(), "the unadmitted immutable defaults do not have this Case's IDs");
        let prepared_a = compiling(&a, || { crate::workspace::validate(&filters).unwrap(); crate::query::prepare(&filters) });
        let mut event = crate::model::Event::empty(); event.message = "CASE_ALPHA".into();
        compiling(&b, || {
            crate::workspace::validate(&filters).unwrap();
            let prepared_b = crate::query::prepare(&filters);
            assert!(prepared_a.iter().all(|filter| crate::query::matches(&event, filter)));
            assert!(!prepared_b.iter().all(|filter| crate::query::matches(&event, filter)));
        });
        assert!(prepared_a.iter().all(|filter| crate::query::matches(&event, filter)), "prepared jobs keep A even after the admitted thread scope ends");
    }

    #[test]
    fn derived_conditions_keep_exact_case_security_in_serial_rayon_and_background_hydration() {
        use rayon::prelude::*;
        let a = custom("CASE_ALPHA"); let b = custom("CASE_BRAVO");
        let derive = |security: &Settings| compiling(security, || {
            let regular = crate::sources::CompiledDerived {
                name: "captured_match".into(), source: "message".into(), steps: vec![], lookup: None,
                rules: vec![crate::sources::CompiledRule::new(regex::Regex::new("(CASE_[A-Z]+)").unwrap(), None,
                    Some(crate::query::Filter { column: "_all".into(), op: "threat_rule".into(), value: "case.signal".into(), value2: None })).unwrap()],
            };
            let mut transformed = regular.clone(); transformed.name = "transformed_match".into(); transformed.steps = vec![crate::field_transform::Step::UrlDecode];
            vec![regular, transformed]
        });
        let fields_a = derive(&a); let fields_b = derive(&b);
        assert_ne!(fields_a[0].rules[0].filter_security_signature, fields_b[0].rules[0].filter_security_signature);
        let root = tempfile::tempdir().unwrap(); let path = root.path().join("same-source.jsonl");
        std::fs::write(&path, "{\"message\":\"CASE_ALPHA\"}\n{\"message\":\"CASE_BRAVO\"}\n").unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = crate::model::CodesConfig::default();
        let read = |id, fields: &[crate::sources::CompiledDerived]| {
            let event = crate::sources::event_at(&index, id, &codes, &codes, fields);
            assert_eq!(event.fields.get("captured_match"), event.fields.get("transformed_match"), "legacy extraction and regex+transform conditions use the same pinned semantics");
            event.fields.get("captured_match").and_then(|v| v.as_str()).map(str::to_owned)
        };
        let expected_a = vec![Some("CASE_ALPHA".to_owned()), None];
        let expected_b = vec![None, Some("CASE_BRAVO".to_owned())];
        assert_eq!((0..2).map(|id| read(id, &fields_a)).collect::<Vec<_>>(), expected_a);
        assert_eq!((0..2).map(|id| read(id, &fields_b)).collect::<Vec<_>>(), expected_b);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let parallel = pool.install(|| (0..2).into_par_iter().map(|id| (read(id, &fields_a), read(id, &fields_b))).collect::<Vec<_>>());
        assert_eq!(parallel, expected_a.iter().cloned().zip(expected_b.iter().cloned()).collect::<Vec<_>>());
        let governed = pool.install(|| crate::global_scheduler::with_limit(2, || {
            crate::operations::run_with_token(crate::operations::token(None).unwrap(), || {
                let barrier = std::sync::Barrier::new(2);
                crate::global_scheduler::map(0..2, |id| {
                    barrier.wait();
                    (read(id, &fields_a), read(id, &fields_b))
                })
            }).unwrap()
        }));
        assert_eq!(governed, expected_a.iter().cloned().zip(expected_b.iter().cloned()).collect::<Vec<_>>(), "governed parallel hydration keeps each predicate's captured Case semantics");
        std::thread::scope(|scope| {
            let first = scope.spawn(|| (0..2).map(|id| read(id, &fields_a)).collect::<Vec<_>>());
            let second = scope.spawn(|| (0..2).map(|id| read(id, &fields_b)).collect::<Vec<_>>());
            assert_eq!(first.join().unwrap(), expected_a); assert_eq!(second.join().unwrap(), expected_b);
        });
        // Build the real DuckDB variants after their compile-time owner scope
        // has ended. The same raw IDs/definition must bake different rows.
        struct DisableEngine;
        impl Drop for DisableEngine { fn drop(&mut self) { crate::engine::set_enabled(false); } }
        let _disable = DisableEngine;
        let definitions = serde_json::json!([
            {"name":"captured_match","source":"message","rules":[{"pattern":"(CASE_[A-Z]+)","filter":{"column":"_all","op":"threat_rule","value":"case.signal"}}]},
            {"name":"transformed_match","source":"message","steps":["url_decode"],"rules":[{"pattern":"(CASE_[A-Z]+)","filter":{"column":"_all","op":"threat_rule","value":"case.signal"}}]}
        ]).to_string();
        let open = |security: &Settings| compiling(security, || crate::testkit::Source::open(&[path.to_str().unwrap()], "{}", &definitions).unwrap());
        let source_a = open(&a); let source_b = open(&b);
        source_a.prepare().unwrap(); source_b.prepare().unwrap();
        for field in ["captured_match", "transformed_match"] {
            let filters = serde_json::json!([{"column":field,"op":"not_empty","value":""}]).to_string();
            assert_eq!(source_a.matches(crate::testkit::Engine::Columnar, &filters), vec![0], "A's baked rows survive B's later publication");
            assert_eq!(source_b.matches(crate::testkit::Engine::Columnar, &filters), vec![1], "same rule ID uses B's own catalog in its baked variant");
            assert_eq!(source_a.matches(crate::testkit::Engine::Columnar, &filters), vec![0], "A→B→A cannot reuse the other Case's semantic variant");
        }
    }

}

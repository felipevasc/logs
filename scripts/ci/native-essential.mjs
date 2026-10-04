// End-to-end data contracts replace the former thousand-test release sweep.
import { spawnSync } from 'node:child_process';

const base = ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--locked'];
const units = [
  'cache_validation::tests::', 'computed_cache::tests::',
  'metadata_checkpoint::tests::', 'index_cache::timestamp_overlay_tests::',
  'engine::build::checkpoint_tests::', 'engine::time_index::tests::',
  'workspace::canonical::tests::', 'triage::on_demand_tests::',
  'security_progress::tests::',
  'source_publication::tests::', 'global_scheduler::tests::',
  'remote::tests::', 'remote_files::tests::', 'field_indexes::tests::',
  'analysis_context::tests::existing_sqlite_cases_migrate_but_first_new_case_does_not_inherit_globals',
  'analysis_context::tests::exported_config_imports_with_new_identity_without_foreign_globals',
  'analysis_context::tests::same_case_id_recreated_cannot_accept_old_analysis_edit',
  'analysis_context::tests::invalid_import_rolls_back_case_body_and_context_together',
  'security_tests::v013_population_pages_preserve_original_membership_and_atomic_saved_metadata',
  'regression_tests::multi_file_loading_preserves_order_and_failed_wave_keeps_previous_source',
];
const commands = process.argv.includes('--extended')
  ? [[...base, '--tests', '--', '--test-threads=1']]
  : [[...base, '--lib', '--', '--test-threads=1', ...units],
     [...base, '--test', 'engine_parity', '--test', 'engine_build', '--test', 'metadata_recovery', '--test', 'startup_reuse', '--test', 'wrapped_log_import', '--', '--test-threads=1']];
for (const args of commands) {
  const result = spawnSync('cargo', args, { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) { process.exitCode = result.status ?? 1; break; }
}

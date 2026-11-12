#!/usr/bin/env ruby
require 'json'
root = File.expand_path('..', __dir__)
evidence = File.join(root, 'evidence')
proofs = {}
%w[initial-generation.log native-generation.log].each do |name|
  File.read(File.join(evidence, name)).scan(/PASS real native PPE ([^:]+): proof_bytes=(\d+) elapsed_ms=(\d+)/).each do |kind, bytes, elapsed|
    proofs[kind] = { 'proof_bytes' => bytes.to_i, 'stage_wall_ms' => elapsed.to_i }
  end
end
rss = lambda do |subdir|
  Dir[File.join(evidence, subdir, 'r0vm.*')].map do |p|
    m = File.read(p).match(/(\d+)\s+maximum resident set size/)
    m && m[1].to_i
  end.compact.max
end
preflight = lambda do |name|
  m = File.read(File.join(evidence, name)).match(/user_cycles=(\d+) segments=(\d+) execution_ms=(\d+) input_bytes=(\d+)/)
  m && { 'user_cycles' => m[1].to_i, 'segments' => m[2].to_i, 'execution_ms' => m[3].to_i, 'input_bytes' => m[4].to_i }
end
result = {
  'date' => '2026-11-18',
  'host' => { 'cpu' => 'Apple M1 Pro', 'logical_cpus' => 10, 'ram_bytes' => 34_359_738_368, 'host_profile' => 'Rust dev, debug=0; official release r0vm 3.1.0' },
  'conditions' => 'Concurrent CPU load; not an isolated throughput benchmark. PPE generation occurred in two runs; corrected suite reverified the first three real receipts. Initial harness scratch-state assertion failure is preserved.',
  'ppe' => proofs,
  'baseline_execution_only' => preflight.call('checkpoint-baseline-preflight.log'),
  'native_framing_execution_only' => preflight.call('checkpoint-framed-preflight.log'),
  'interrupted_checkpoint_attempt' => JSON.parse(File.read(File.join(evidence, 'checkpoint-interrupted-status.json'))),
  'baseline_proving_status' => 'Stopped after profiling, no receipt obtained; not positive proof evidence',
  'checkpoint_proof' => JSON.parse(File.read(File.join(evidence, 'checkpoint.json'))),
  'checkpoint_reverification' => JSON.parse(File.read(File.join(evidence, 'checkpoint-verification.json'))),
  'native_authentication' => JSON.parse(File.read(File.join(evidence, 'native-auth.json'))),
  'max_r0vm_rss_bytes' => { 'fourth_ppe_generation' => rss.call('corrected-prover-resources'), 'checkpoint_generation' => rss.call('checkpoint-retry-prover-resources') },
  'full_source_history_cost_measured' => false,
  'ethereum_gas_measured' => false
}
File.write(File.join(evidence, 'metrics.json'), JSON.pretty_generate(result) + "\n")
puts 'Wrote evidence/metrics.json from actual logs and receipts'

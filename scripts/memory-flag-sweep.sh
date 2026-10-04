#!/usr/bin/env bash
# Runs the agent memory eval once per CortexDB flag profile
# (integration/cortexdb/flags/*.env), each against its own throwaway server,
# then compares the runs' KPIs against the baseline profile.
# See docs/evals/cortex-flags.md.
#
#   ./scripts/memory-flag-sweep.sh                      # every profile, mock models
#   ./scripts/memory-flag-sweep.sh baseline no-graph    # just these
#   MODELS=openrouter ./scripts/memory-flag-sweep.sh -- --llm
#   REPEAT=2 PARALLEL=4 ./scripts/memory-flag-sweep.sh  # repeats show the noise
#   ./scripts/memory-flag-sweep.sh -- --only conflicts,surprise
#
# Arguments before `--` name profiles; after it, they go to the eval.
# MODELS, CORTEXDB_VERSION and the model overrides pass through to
# memory-eval.sh. PARALLEL servers run at once, on ports from BASE_PORT up.
# Reports, logs and summary.md land in OUT_DIR (target/memory-eval/flags).
#
# MODELS=openrouter spends money on every profile: measure the baseline's
# cost first (its report ends with it) and multiply.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
flags="$root/integration/cortexdb/flags"
repeat="${REPEAT:-1}"
parallel="${PARALLEL:-1}"
base_port="${BASE_PORT:-3150}"
out="${OUT_DIR:-$root/target/memory-eval/flags}"

profiles=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
  profiles+=("$1")
  shift
done
[ "${1:-}" = "--" ] && shift
if [ ${#profiles[@]} -eq 0 ]; then
  for file in "$flags"/*.env; do
    profiles+=("$(basename "$file" .env)")
  done
fi

mkdir -p "$out"
# Build once, so parallel runs do not queue on cargo's lock.
cargo build --quiet -p tinymemory-integrations --features full --example memory_eval

# Runs `profile`, repeat `n`, on `port`.
run_one() {
  local profile="$1" n="$2" port="$3"
  local label="flags-$profile-r$n"
  (
    # Sourced as well as passed to Compose: the profile may set what the
    # compose file interpolates (CORTEX_TOML, CORTEX_VERIFIER_URL).
    set -a
    # shellcheck disable=SC1090
    . "$flags/$profile.env"
    set +a
    CORTEX_FLAGS_FILE="$flags/$profile.env" CORTEXDB_PORT="$port" LABEL="$label" \
      OUT_DIR="$out" "$root/scripts/memory-eval.sh" "$@"
  ) >"$out/$label.log" 2>&1
}

jobs_run=()
failed=()
port="$base_port"
running=0
for profile in "${profiles[@]}"; do
  file="$flags/$profile.env"
  if [ ! -f "$file" ]; then
    echo "no profile $profile in $flags" >&2
    exit 1
  fi
  needs="$(sed -n 's/^# requires: *//p' "$file")"
  if [ -n "$needs" ] && [ -z "${!needs:-}" ]; then
    echo "skip $profile: needs $needs"
    continue
  fi
  for n in $(seq 1 "$repeat"); do
    label="flags-$profile-r$n"
    echo "run  $label on :$port"
    (run_one "$profile" "$n" "$port" "$@" && echo "done $label" || {
      echo "FAIL $label (see $out/$label.log)"
      exit 1
    }) &
    jobs_run+=("$label")
    port=$((port + 1))
    running=$((running + 1))
    if [ "$running" -ge "$parallel" ]; then
      wait -n || failed+=("one run")
      running=$((running - 1))
    fi
  done
done
while [ "$running" -gt 0 ]; do
  wait -n || failed+=("one run")
  running=$((running - 1))
done

reports=()
for label in "${jobs_run[@]}"; do
  [ -f "$out/$label.json" ] && reports+=("$out/$label.json")
done
if [ ${#reports[@]} -eq 0 ]; then
  echo "no run finished; see the logs in $out" >&2
  exit 1
fi
cargo run --quiet -p tinymemory-integrations --features full --example memory_eval -- \
  compare "${reports[@]}" | tee "$out/summary.md"
echo
echo "wrote $out/summary.md"
if [ ${#failed[@]} -gt 0 ]; then
  echo "${#failed[@]} run(s) failed; see the logs in $out" >&2
  exit 1
fi

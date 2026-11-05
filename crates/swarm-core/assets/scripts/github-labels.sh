#!/usr/bin/env bash
# A pinned copy from swarm-core (crates/swarm-core/assets/scripts/github-labels.sh): edit it
# there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`. The project's
# own labels (its capabilities and areas) are in scripts/github-labels.project.sh, which this runs
# after the shared ones.
#
# Create (or update) the issue labels the work-tracking flow uses (docs/swarm-protocol.md).
# Idempotent; needs only gh, authenticated against {{github.host}}:
#   gh auth login --hostname {{github.host}}
#
# Usage: scripts/github-labels.sh [owner/repo]   (default: {{github.repo}})
set -euo pipefail
REPO="${1:-{{github.repo}}}"
export GH_HOST="${GH_HOST:-{{github.host}}}"

label() { # name color description
  gh label create "$1" --repo "$REPO" --color "${2#\#}" --description "$3" --force >/dev/null
  echo "label  $1"
}

# Claims: the one in-flight signal agents check before starting (with the issue's open PRs).
label "status::in-progress" "#1f75cb" "Claimed: a draft PR is open for it (see the issue's comments)"
label "status::blocked"     "#dc143c" "Waiting on something outside the claimer's control; the comment says what"
label "sync"                "#8e44ad" "A twin of another swarm's change, to port here (docs/swarm-protocol.md, step 9)"
label "needs::human"        "#ed9121" "An agent can't finish this alone (decision, credentials, hardware, permissions)"
label "main-red"            "#b22222" "main is red: nothing merges or starts from main until it is green (swarm-protocol.md, step 3)"
# Kinds.
label "type::task" "#6699cc" "Planned work"
label "type::bug"  "#d9534f" "Behaves wrongly or crashes"
label "type::gap"  "#8e44ad" "Tracks a gap entry in docs/gaps.md"
# CI.
label "full-ci" "#c0c0c0" "Run every platform job on this pull request"

# The project's own labels, defined with `label` above.
PROJECT_LABELS="$(dirname "$0")/github-labels.project.sh"
# shellcheck source=/dev/null
if [ -f "$PROJECT_LABELS" ]; then
  . "$PROJECT_LABELS"
fi

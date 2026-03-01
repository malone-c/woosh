#!/bin/bash
# Stop hook: block Claude from finishing if source files changed but docs weren't updated.
#
# Claude Code calls this when the model tries to produce its final response.
# Exit 0 with JSON {"decision":"block","reason":"..."} → Claude continues working.
# Exit 0 with no JSON                                  → Claude stops normally.
# The `stop_hook_active: true` field in input prevents infinite loops.

set -euo pipefail

INPUT=$(cat)

# If this is already a continuation turn, let Claude stop — prevent infinite loop.
if [ "$(echo "$INPUT" | jq -r '.stop_hook_active // false')" = "true" ]; then
  exit 0
fi

# Collect all files modified or added since the last commit (staged + unstaged + untracked).
CHANGED=$(git status --porcelain 2>/dev/null | awk '{print $2}')

# Check whether any source code files changed.
SOURCE_CHANGED=$(echo "$CHANGED" | grep -E '^src/.*\.(rs|toml)$' || true)

if [ -z "$SOURCE_CHANGED" ]; then
  exit 0  # No source changes — docs update not required.
fi

# Check whether any documentation files were updated.
DOCS_CHANGED=$(echo "$CHANGED" | grep -E '(ROADMAP|AGENTS|SPEC)\.md$' || true)

if [ -z "$DOCS_CHANGED" ]; then
  printf '{"decision":"block","reason":"Source files were modified but documentation was not updated. Per project conventions (AGENTS.md Workflow section): update ROADMAP.md (check off completed tasks with [x]), update the AGENTS.md in any directory where code changed, and update the root AGENTS.md and SPEC.md if needed. Please do this now before finishing."}\n'
  exit 0
fi

exit 0

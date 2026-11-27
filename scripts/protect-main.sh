#!/usr/bin/env bash
# Applies (and re-applies) the `main` protection rules. Idempotent: safe to run on every
# clone and after recreating the repository, because the rules live on GitHub, not in the
# repository itself.
#
# A pull request is still required: a present `required_pull_request_reviews` with an
# approval count of zero switches that requirement on, and it is the only thing that stops a
# direct push — required status checks gate merges, never pushes. `enforce_admins` binds the
# owner to the same rules, and no approval is asked because GitHub forbids self-approval.
#
# Usage:
#   scripts/protect-main.sh                 # base rules, keeping any required CI checks
#   scripts/protect-main.sh --require-ci    # base rules plus the `ci` check, required
#
# Requires the `gh` CLI authenticated with admin rights on the repository.
set -euo pipefail

REQUIRE_CI=0
REPO=""
for argument in "$@"; do
  case "$argument" in
    --require-ci) REQUIRE_CI=1 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    -*) printf 'error: unknown option: %s\n' "$argument" >&2; exit 1 ;;
    *) REPO="$argument" ;;
  esac
done

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null 2>&1 || die "the gh CLI is required"
command -v python3 >/dev/null 2>&1 || die "python3 is required to build the payload"
gh auth status >/dev/null 2>&1 || die "gh is not authenticated; run 'gh auth login'"
[ -n "$REPO" ] || REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
BRANCH="main"

say "repository: $REPO"
say "branch    : $BRANCH"

# A code-owner rule without a resolvable owner blocks every pull request permanently,
# so the prerequisites are checked before anything is applied.
gh api "repos/$REPO/contents/.github/CODEOWNERS" >/dev/null 2>&1 ||
  die ".github/CODEOWNERS is missing: nobody would be requested for review on a pull request"
say "CODEOWNERS: present"

errors="$(gh api "repos/$REPO/codeowners/errors" --jq '.errors | length')"
[ "$errors" = "0" ] || die "CODEOWNERS has $errors error(s): gh api repos/$REPO/codeowners/errors"
say "CODEOWNERS: no errors"

if [ "$REQUIRE_CI" = 1 ]; then
  conclusion="$(gh api "repos/$REPO/commits/$BRANCH/check-runs" \
    --jq '.check_runs[] | select(.name == "ci") | .conclusion' 2>/dev/null | head -1)"
  [ "$conclusion" = "success" ] ||
    die "the 'ci' check has no successful run on $BRANCH (last: ${conclusion:-none}); a required check that never reports blocks every pull request"
  say "CI check 'ci': has a green run"
fi

# The payload is built by python so the required checks can be read from GitHub and
# preserved: re-running this script must never silently drop a CI requirement.
payload="$(REPO="$REPO" BRANCH="$BRANCH" REQUIRE_CI="$REQUIRE_CI" python3 - <<'PY'
import json
import os
import subprocess

repo = os.environ["REPO"]
branch = os.environ["BRANCH"]

contexts: list[str] = []
probe = subprocess.run(
    ["gh", "api", f"repos/{repo}/branches/{branch}/protection"],
    capture_output=True,
    text=True,
    check=False,
)
if probe.returncode == 0:
    current = json.loads(probe.stdout or "{}") or {}
    contexts = list((current.get("required_status_checks") or {}).get("contexts") or [])

if os.environ["REQUIRE_CI"] == "1" and "ci" not in contexts:
    contexts.append("ci")

payload = {
    # A pull request stays required. A present `required_pull_request_reviews` is what turns
    # GitHub's "Require a pull request before merging" on, and a count of zero asks for no
    # approval: GitHub forbids approving your own pull request, so a count of one would
    # freeze a single maintainer forever. `enforce_admins` binds the owner to the same rules.
    "enforce_admins": True,
    "required_pull_request_reviews": {
        "dismiss_stale_reviews": True,
        "require_code_owner_reviews": False,
        "required_approving_review_count": 0,
        "require_last_push_approval": False,
    },
    "required_status_checks": {"strict": True, "contexts": contexts} if contexts else None,
    "restrictions": None,
    "required_linear_history": False,
    "allow_force_pushes": False,
    "allow_deletions": False,
    "required_conversation_resolution": True,
    "block_creations": False,
    "lock_branch": False,
}
print(json.dumps(payload))
PY
)"

say ""
say "applying protection to $BRANCH…"
printf '%s' "$payload" |
  gh api --method PUT "repos/$REPO/branches/$BRANCH/protection" --input - >/dev/null
say "done"

gh api --method PATCH "repos/$REPO" -F has_issues=true -F delete_branch_on_merge=true >/dev/null
say "issues enabled, branches deleted after merge"

say ""
say "effective configuration:"
gh api "repos/$REPO/branches/$BRANCH/protection" |
  python3 -c '
import json, sys
d = json.load(sys.stdin)
reviews = d.get("required_pull_request_reviews")
checks = d.get("required_status_checks") or {}
contexts = checks.get("contexts") or []
# A present `required_pull_request_reviews` is what turns "Require a pull request before
# merging" on, and that is the only thing that stops a direct push; a required check gates
# the merge of a pull request somebody already opened.
gate = reviews is not None
print("  pull request required       :", "yes" if gate else "NO")
print("  required CI checks          :", contexts or "none")
print("  required approvals          :", (reviews or {}).get("required_approving_review_count", "none"))
print("  owner also restricted       :", d["enforce_admins"]["enabled"])
print("  force pushes allowed        :", d["allow_force_pushes"]["enabled"])
print("  deleting the branch allowed :", d["allow_deletions"]["enabled"])
print("  branch must be up to date   :", checks.get("strict", "-"))
if not gate:
    sys.stderr.write("error: the applied rule does not require a pull request\n")
    sys.exit(1)
'

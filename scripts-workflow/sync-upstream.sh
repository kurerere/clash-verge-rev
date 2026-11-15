#!/usr/bin/env bash
set -euo pipefail

before=$(git rev-parse HEAD)
identity_filter='{identifier, updater: .plugins.updater | {pubkey, endpoints}}'
identity=$(jq -Sc "$identity_filter" src-tauri/tauri.conf.json)

git fetch --no-tags https://github.com/clash-verge-rev/clash-verge-rev.git dev
upstream=$(git rev-parse FETCH_HEAD)
if git merge-base --is-ancestor "$upstream" HEAD; then
  echo 'Upstream dev is already merged; no release needed.' >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi

if ! git merge --no-edit "$upstream"; then
  echo '::error::Upstream merge failed. Resolve conflicts manually; no branch or tag was pushed.'
  git diff --name-only --diff-filter=U
  exit 1
fi

if [[ "$(jq -Sc "$identity_filter" src-tauri/tauri.conf.json)" != "$identity" ]]; then
  echo '::error::Upstream changed the fork identifier, updater key or endpoints. Review manually; nothing was pushed.'
  exit 1
fi

new_workflows=$(git diff --name-only --diff-filter=A "$before" HEAD -- .github/workflows)
if [[ -n "$new_workflows" ]]; then
  echo '::error::New upstream workflows require manual review before syncing so upstream publishing is not enabled accidentally.'
  printf '%s\n' "$new_workflows"
  exit 1
fi

if ! git diff --quiet "$before" HEAD -- .github/workflows/fork-release.yml; then
  echo '::error::The fork release workflow changed. Review manually; nothing was pushed.'
  exit 1
fi

version=$(git show "$upstream:package.json" | jq -er '.version')
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo '::error::Upstream package version must be X.Y.Z.'
  exit 1
fi

tag=$(python3 - "$version" <<'PY'
import re
import subprocess
import sys

version = sys.argv[1]
base = tuple(map(int, version.split('.')))
releases = []
for tag in subprocess.check_output(['git', 'tag', '--list', 'v*-kure.*'], text=True).splitlines():
    match = re.fullmatch(r'v(\d+)\.(\d+)\.(\d+)-kure\.(\d+)', tag)
    if match:
        releases.append(tuple(map(int, match.groups())))
if releases and max(releases)[:3] > base:
    sys.exit('Refusing to publish an older upstream base version than existing fork tags.')
number = max((r[3] for r in releases if r[:3] == base), default=0) + 1
print(f'v{version}-kure.{number}')
PY
)

git tag -a "$tag" -m "Fork release $tag after syncing upstream/dev"
# The PAT triggers Fork Release; an atomic push cannot leave dev synced without a tag.
git push --atomic origin HEAD:refs/heads/dev "refs/tags/$tag"
printf 'Merged upstream %s and pushed %s to trigger Fork Release.\n' "$upstream" "$tag" >> "$GITHUB_STEP_SUMMARY"

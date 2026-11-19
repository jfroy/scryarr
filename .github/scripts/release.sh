#!/usr/bin/env bash
# Bump scryarr's patch version, commit every working-tree change to main
# through the GitHub API, tag the commit and dispatch the release workflow.
#
# main requires verified signatures; commits created with GITHUB_TOKEN through
# the GraphQL createCommitOnBranch mutation are signed by GitHub. Pushes made
# with GITHUB_TOKEN do not trigger other workflows, so the release workflow is
# dispatched explicitly.
#
# Usage: release.sh [commit headline]   (default: "chore: release X.Y.Z")
# Needs GH_TOKEN and GITHUB_REPOSITORY. Writes next=X.Y.Z to $GITHUB_OUTPUT
# when it is set.
set -euo pipefail

current="$(sed -n '0,/^version = "/s/^version = "\(.*\)"$/\1/p' Cargo.toml)"
IFS=. read -r major minor patch <<< "$current"
next="${major}.${minor}.$((patch + 1))"
headline="${1:-chore: release ${next}}"

if git rev-parse -q --verify "refs/tags/v${next}" >/dev/null; then
  echo "tag v${next} already exists" >&2
  exit 1
fi

sed -i "0,/^version = \"${current}\"$/s//version = \"${next}\"/" Cargo.toml
sed -i "/^name = \"scryarr\"$/{n;s/^version = \"${current}\"$/version = \"${next}\"/}" Cargo.lock
grep -q "^version = \"${next}\"$" Cargo.toml
grep -A1 '^name = "scryarr"$' Cargo.lock | grep -qx "version = \"${next}\""

work="$(mktemp -d)"
: > "${work}/additions.jsonl"
for path in $(git diff --name-only); do
  base64 -w0 "$path" > "${work}/blob"
  jq -cn --arg path "$path" --rawfile contents "${work}/blob" \
    '{path: $path, contents: $contents}' >> "${work}/additions.jsonl"
done
jq -n \
  --arg repo "$GITHUB_REPOSITORY" \
  --arg head "$(git rev-parse HEAD)" \
  --arg headline "$headline" \
  --slurpfile additions "${work}/additions.jsonl" \
  '{
    query: "mutation($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid } } }",
    variables: {input: {
      branch: {repositoryNameWithOwner: $repo, branchName: "main"},
      expectedHeadOid: $head,
      message: {headline: $headline},
      fileChanges: {additions: $additions}
    }}
  }' > "${work}/request.json"
oid="$(gh api graphql --input "${work}/request.json" --jq '.data.createCommitOnBranch.commit.oid')"
if [ -z "$oid" ]; then
  echo "createCommitOnBranch returned no commit" >&2
  exit 1
fi

gh api "repos/${GITHUB_REPOSITORY}/git/refs" -f ref="refs/tags/v${next}" -f sha="$oid" > /dev/null
gh workflow run release.yaml --ref "v${next}" -f version="${next}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "next=${next}" >> "$GITHUB_OUTPUT"
fi
echo "released v${next} at ${oid}"

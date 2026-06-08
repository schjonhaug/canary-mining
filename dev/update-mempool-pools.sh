#!/usr/bin/env bash
set -euo pipefail

repo="https://github.com/mempool/mining-pools.git"
raw_base="https://raw.githubusercontent.com/mempool/mining-pools"
branch="${1:-master}"
out_dir="assets/mempool-pools"
json_path="${out_dir}/pools-v2.json"
meta_path="${out_dir}/pools-v2.meta.json"
logo_dir="ui/pool-logos"
logo_meta_path="${out_dir}/pool-logos.meta.json"
logo_base_url="https://mempool.space/resources/mining-pools"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

require_cmd curl
require_cmd git
require_cmd jq
require_cmd shasum
require_cmd tr
require_cmd grep
require_cmd sed

mkdir -p "$out_dir" "$logo_dir"
commit="$(git ls-remote "$repo" "refs/heads/${branch}" | awk '{print $1}')"
if [[ -z "$commit" ]]; then
  echo "Could not resolve ${repo} ${branch}" >&2
  exit 1
fi

tmp_json="$(mktemp)"
trap 'rm -f "$tmp_json"' EXIT

curl -fsS "${raw_base}/${commit}/pools-v2.json" -o "$tmp_json"
jq -e 'type == "array" and all(.[]; (.name | type == "string") and (.addresses | type == "array") and (.tags | type == "array"))' "$tmp_json" >/dev/null

mv "$tmp_json" "$json_path"
trap - EXIT
sha="$(shasum -a 256 "$json_path" | awk '{print $1}')"
fetched_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

jq -n \
  --arg source "https://github.com/mempool/mining-pools" \
  --arg file "pools-v2.json" \
  --arg branch "$branch" \
  --arg commit "$commit" \
  --arg fetched_at "$fetched_at" \
  --arg sha256 "$sha" \
  '{source: $source, file: $file, branch: $branch, commit: $commit, fetched_at: $fetched_at, sha256: $sha256}' \
  > "$meta_path"


slugify_pool_name() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -cd '[:alnum:]'
}

echo "Updating pool logos from ${logo_base_url}..."
logos_tmp="$(mktemp)"
printf '{"source":"%s","fetched_at":"%s","logos":{' "$logo_base_url" "$fetched_at" > "$logos_tmp"
first_logo=1
logo_count=0
while IFS= read -r pool_name; do
  slug="$(slugify_pool_name "$pool_name")"
  if [[ -z "$slug" ]]; then
    continue
  fi
  logo_path="${logo_dir}/${slug}.svg"
  tmp_logo="$(mktemp)"
  if curl -fsS "${logo_base_url}/${slug}.svg" -o "$tmp_logo" 2>/dev/null; then
    if grep -qi '<svg' "$tmp_logo"; then
      mv "$tmp_logo" "$logo_path"
      logo_sha="$(shasum -a 256 "$logo_path" | awk '{print $1}')"
      if [[ "$first_logo" -eq 0 ]]; then
        printf ',' >> "$logos_tmp"
      fi
      first_logo=0
      logo_count=$((logo_count + 1))
      jq -n --arg slug "$slug" --arg name "$pool_name" --arg sha256 "$logo_sha" \
        '[$slug, {name: $name, file: ($slug + ".svg"), sha256: $sha256}]' \
        | jq -c '.[0] as $key | {($key): .[1]}' \
        | sed 's/^{//; s/}$//' >> "$logos_tmp"
    else
      rm -f "$tmp_logo"
    fi
  else
    rm -f "$tmp_logo"
  fi
done < <(jq -r '.[].name' "$json_path")
printf '},"count":%s}\n' "$logo_count" >> "$logos_tmp"
jq . "$logos_tmp" > "$logo_meta_path"
rm -f "$logos_tmp"
echo "Updated ${logo_count} pool logos in ${logo_dir}"

echo "Updated ${json_path} from ${commit}"
echo "sha256 ${sha}"

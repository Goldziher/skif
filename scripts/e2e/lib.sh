#!/usr/bin/env bash
# The package-manager tables and the scenario generator of the end-to-end tests, sourced by run.sh and by
# scripts/test-e2e-scripts.sh. Nothing here installs or touches the network.
#
# A scenario is a directory under fixtures/e2e/ with
#   scenario.json      which managers it applies to, the roots to install, an optional catalog, the expected findings
#   project/           the files shared by every manager, the root `package.json` declaring `workspaces`
#   overlays/<pm>/     optional files for one manager: copied over the project, except `<name>.merge.json`, which is
#                      deep-merged into `<name>.json` (a null value deletes the key)
# The generator writes the manager's own forms (pnpm-workspace.yaml, the catalog syntax, the linker setting, the
# `packageManager` pin of the version under test), so the shared files hold no manager-specific syntax.
#
# The managers are npm, pnpm, yarn (Yarn 2 and later), yarn-classic (Yarn 1.x) and bun.
# Every function returns non-zero on the first failing step: callers run them under `||` or `if`, where `set -e` is
# off, so each step propagates its own failure.
# The jq programs contain `$` on purpose; nothing is meant to expand.
# shellcheck disable=SC2016

# jq with carriage returns removed, so values read on Windows compare equal.
jqr() {
  jq "$@" | tr -d '\r'
}

# e2e_command <pm>: the executable of a manager.
e2e_command() {
  case "$1" in
  yarn-classic) echo yarn ;;
  npm | pnpm | yarn | bun) echo "$1" ;;
  *)
    echo "unsupported package manager: $1" >&2
    return 2
    ;;
  esac
}

# e2e_linkers <pm>: the dependency layouts a manager is tested with; the first is its default.
#   pnpm   isolated (the default) and node-linker hoisted
#   yarn   pnp (the default, no .yarnrc.yml), node-modules and pnpm
#   bun    default (no bunfig.toml: isolated for a new workspace, which skif reads from bun.lock's configVersion),
#          and the hoisted and isolated linkers set in bunfig.toml
#   npm, yarn-classic   hoisted, their only layout
e2e_linkers() {
  case "$1" in
  npm | yarn-classic) echo hoisted ;;
  pnpm) echo "isolated hoisted" ;;
  yarn) echo "pnp node-modules pnpm" ;;
  bun) echo "default hoisted isolated" ;;
  *)
    echo "unsupported package manager: $1" >&2
    return 2
    ;;
  esac
}

# e2e_layout <pm> <linker>: what an install with the linker leaves in the root, as `<hoisted> <marker>`: whether the
# members' dependencies are hoisted into the root node_modules, and a path only that layout creates ("-" for none).
e2e_layout() {
  case "$1/$2" in
  npm/hoisted | yarn-classic/hoisted | bun/hoisted | pnpm/hoisted | yarn/node-modules) echo "yes -" ;;
  pnpm/isolated) echo "no node_modules/.pnpm" ;;
  yarn/pnp) echo "no .pnp.cjs" ;;
  yarn/pnpm) echo "no node_modules/.store" ;;
  bun/default | bun/isolated) echo "no node_modules/.bun" ;;
  *)
    echo "unknown linker '$2' for $1" >&2
    return 2
    ;;
  esac
}

# e2e_install <pm> <dir>: installs a generated root. On CI, pnpm and Yarn Berry default to a frozen lockfile; the
# fixtures commit none, so their installs are allowed to create one.
e2e_install() {
  local pm="$1" dir="$2"
  case "$pm" in
  npm) (cd "$dir" && npm install --ignore-scripts --no-audit --no-fund) ;;
  pnpm) (cd "$dir" && pnpm install --ignore-scripts --no-frozen-lockfile) ;;
  yarn) (cd "$dir" && YARN_ENABLE_IMMUTABLE_INSTALLS=false yarn install --mode=skip-build) ;;
  yarn-classic) (cd "$dir" && yarn install --ignore-scripts --non-interactive --no-progress) ;;
  bun) (cd "$dir" && bun install --ignore-scripts) ;;
  *)
    echo "unsupported package manager: $pm" >&2
    return 2
    ;;
  esac
}

# e2e_json_edit <file> <jq args...>: rewrites a JSON file in place with a jq filter, with LF line endings on every OS.
e2e_json_edit() {
  local file="$1"
  shift
  e2e_replace "$file" jqr "$@" "$file"
}

# e2e_replace <file> <command...>: replaces the file with the command's output, only when the command succeeds.
e2e_replace() {
  local file="$1"
  shift
  if ! "$@" >"${file}.tmp"; then
    rm -f "${file}.tmp"
    return 1
  fi
  mv "${file}.tmp" "$file"
}

# e2e_applies <scenario-dir> <pm> <version>: prints why a scenario does not apply to the manager at that version, and
# nothing when it does. `managers` lists the managers (default: all); `requires` maps a manager to its minimum version.
e2e_applies() {
  jqr -r --arg pm "$2" --arg version "$3" '
    def parts: split(".") | map(tonumber);
    if (.managers // [$pm] | index($pm)) == null then "not a \($pm) scenario"
    elif .requires[$pm] != null and ($version | parts) < (.requires[$pm] | parts) then
      "needs \($pm) \(.requires[$pm]) or later"
    else empty end' "$1/scenario.json"
}

# e2e_expectation <scenario-dir> <pm> <linker>: the expected outcome as JSON: `expect`, then the overrides for the
# manager and for `<pm>/<linker>`. An override sets `exit_code`, `add`s and `remove`s findings, and holds the same for
# the `after_fix` and `migrate` steps; removing a finding the expectation lacks is an error, so an override cannot go
# stale silently. An override's `run` is merged into the expected script run, and its `known_issue` is collected into
# `known_issues`. Other keys, such as `reason`, are notes.
e2e_expectation() {
  jq -c --arg pm "$2" --arg key "$2/$3" '
    def apply($o):
      if $o == null then .
      else
        (($o.remove // []) - (.diagnostics // [])) as $stale
        | if $stale != [] then error("override removes absent findings: \($stale | join(", "))") else . end
        | (if $o.exit_code != null then .exit_code = $o.exit_code else . end)
        | .diagnostics = (((.diagnostics // []) + ($o.add // [])) - ($o.remove // []) | sort)
        | (if $o.run != null then .run += $o.run else . end)
        | (if $o.known_issue != null then .known_issues += [$o.known_issue] else . end)
        | reduce ("after_fix", "migrate") as $step (.;
            if $o[$step] != null then .[$step] |= apply($o[$step]) else . end)
      end;
    def sorted: .diagnostics = ((.diagnostics // []) | sort);
    .overrides as $overrides
    | .expect
    | sorted
    | reduce ("after_fix", "migrate") as $step (.; if .[$step] != null then .[$step] |= sorted else . end)
    | apply($overrides[$pm]) | apply($overrides[$key])' "$1/scenario.json"
}

# e2e_overlay <overlay-dir> <dest>: applies a manager's overlay to a generated project.
e2e_overlay() {
  local src="$1" dest="$2" rel target files
  files="$(cd "$src" && find . -type f | LC_ALL=C sort)" || return
  while IFS= read -r rel; do
    rel="${rel#./}"
    case "$rel" in
    *.merge.json)
      target="${dest}/${rel%.merge.json}.json"
      [[ -f "$target" ]] || {
        echo "overlay ${src}/${rel} merges into a missing ${target}" >&2
        return 1
      }
      e2e_replace "$target" jqr -s '.[0] * .[1] | delpaths([paths(. == null)])' "$target" "${src}/${rel}" || return
      ;;
    *)
      mkdir -p "$(dirname "${dest}/${rel}")" && cp "${src}/${rel}" "${dest}/${rel}" || return
      ;;
    esac
  done <<<"$files"
}

# e2e_root <dir> <pm> <linker> <version>: pins the manager of one root and writes its workspace and linker settings.
e2e_root() {
  local dir="$1" pm="$2" linker="$3" version="$4" manifest="$1/package.json" pin members
  pin="$(e2e_command "$pm")@${version}" || return
  e2e_json_edit "$manifest" --arg pin "$pin" '.packageManager = $pin' || return
  case "$pm" in
  pnpm)
    # pnpm reads its members and settings from pnpm-workspace.yaml, never from `workspaces`. Every root gets one, so
    # the linker is set explicitly and a standalone package below a workspace is its own pnpm root.
    members="$(jqr -r '(.workspaces | if type == "object" then .packages else . end) // [] | .[]' "$manifest")" || return
    {
      if [[ -n "$members" ]]; then
        echo "packages:"
        while IFS= read -r member; do printf "  - '%s'\n" "$member"; done <<<"$members"
      fi
      echo "nodeLinker: ${linker}"
    } >>"${dir}/pnpm-workspace.yaml" || return
    e2e_json_edit "$manifest" 'del(.workspaces)'
    ;;
  yarn)
    # Yarn Berry treats a directory as its own project only with a lockfile; pnp is its default and needs no setting.
    : >>"${dir}/yarn.lock" || return
    if [[ "$linker" != pnp ]]; then
      printf 'nodeLinker: %s\n' "$linker" >>"${dir}/.yarnrc.yml"
    fi
    ;;
  bun)
    if [[ "$linker" != default ]]; then
      printf '[install]\nlinker = "%s"\n' "$linker" >>"${dir}/bunfig.toml"
    fi
    ;;
  esac
}

# e2e_catalog <dir> <pm> <catalog-json>: declares the default catalog in the manager's own form.
e2e_catalog() {
  local dir="$1" pm="$2" catalog="$3" file
  case "$pm" in
  pnpm | yarn)
    file="${dir}/pnpm-workspace.yaml"
    [[ "$pm" == yarn ]] && file="${dir}/.yarnrc.yml"
    {
      echo "catalog:"
      jqr -r 'to_entries[] | "  \(.key | tojson): \(.value | tojson)"' <<<"$catalog"
    } >>"$file" || return
    ;;
  bun)
    e2e_json_edit "${dir}/package.json" --argjson catalog "$catalog" \
      '.workspaces = {packages: (.workspaces | if type == "object" then .packages else . end), catalog: $catalog}'
    ;;
  *)
    echo "${pm} has no catalogs" >&2
    return 1
    ;;
  esac
}

# e2e_roots <scenario-dir>: the roots of a scenario relative to its project, one per line (default: the project).
e2e_roots() {
  jqr -r '.roots // ["."] | .[]' "$1/scenario.json"
}

# e2e_generate <scenario-dir> <pm> <linker> <version> <dest>: writes the scenario's project for one manager, linker
# and version into dest.
e2e_generate() {
  local scenario="$1" pm="$2" linker="$3" version="$4" dest="$5" root roots catalog
  mkdir -p "$dest" && cp -R "${scenario}/project/." "$dest/" || return
  if [[ -d "${scenario}/overlays/${pm}" ]]; then
    e2e_overlay "${scenario}/overlays/${pm}" "$dest" || return
  fi
  roots="$(e2e_roots "$scenario")" || return
  while IFS= read -r root; do
    e2e_root "${dest}/${root}" "$pm" "$linker" "$version" || return
  done <<<"$roots"
  catalog="$(jq -c '.catalog // empty' "${scenario}/scenario.json")" || return
  if [[ -n "$catalog" ]]; then
    e2e_catalog "$dest" "$pm" "$catalog" || return
  fi
}

# e2e_findings <skif-json>: the findings of a JSON report as sorted `rule:package[:dependency][ in file]` strings; the
# file is named when it is not a `package.json`, so two findings in different files of one package stay apart.
e2e_findings() {
  jqr -c '[.diagnostics[]
    | ([.rule, .package, .dependency] | map(select(. != null)) | join(":"))
      + (.location.path // ""
        | if . == "" or . == "package.json" or endswith("/package.json") then "" else " in \(.)" end)
    ] | sort' <<<"$1"
}

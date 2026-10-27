#!/usr/bin/env bash
# Runs puml-extract diff, annotates the lines that make the architecture worse, writes the job summary and
# keeps one comment on the pull request up to date.
set -euo pipefail

marker='<!-- puml-diff -->'
limit=65000
directory="$RUNNER_TEMP/puml"
report="$directory/diff.md"
json="$directory/diff.json"
comment="$directory/comment.md"

base=${PUML_BASE:-${GITHUB_BASE_REF:+origin/$GITHUB_BASE_REF}}

if [ -z "$base" ]; then
    echo "::error::Set the base input: only pull_request events have a base to compare with."
    exit 1
fi

mkdir -p "$directory"

status=0
puml-extract diff "$PUML_DIRECTORY" "$base" --format markdown --strict --repository "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY" > "$report" \
    || status=$?

if [ "$status" -gt 1 ]; then
    exit "$status"
fi

puml-extract diff "$PUML_DIRECTORY" "$base" --format json > "$json"

level=$([ "$PUML_STRICT" = true ] && echo error || echo warning)
prefix=$(git -C "$PUML_DIRECTORY" rev-parse --show-prefix)

jq -r --arg level "$level" --arg prefix "$prefix" '
    def message: gsub("%"; "%25") | gsub("\r"; "%0D") | gsub("\n"; "%0A");
    def property: message | gsub(":"; "%3A") | gsub(","; "%2C");
    def annotate($title; $text): .evidence[0] as $place
        | "::\($level) file=\($prefix + $place.file | property),line=\($place.line),title=\($title | property)::\($text | message)";
    def kind: .kind + (if .label then " via " + .label else "" end);

    (.violations.added[] | .rule as $rule | .edge
        | annotate("puml: " + $rule; "\(.from) now depends on \(.to) (\(kind)), which breaks \($rule).")),
    (.dangling[] | .class as $class | .used_by[]
        | annotate("puml: removed class still used"; "This pull request removes \($class), but \(.from) still uses it (\(kind))."))
' "$json"

worsens=$([ "$status" -eq 1 ] && echo true || echo false)
echo "worsens=$worsens" >> "$GITHUB_OUTPUT"
echo "report=$report" >> "$GITHUB_OUTPUT"
cat "$report" >> "$GITHUB_STEP_SUMMARY"

if [ "$PUML_COMMENT" = true ] && [ -n "${PULL_REQUEST:-}" ]; then
    { echo "$marker"; cat "$report"; } > "$comment"

    if [ "$(wc -c < "$comment")" -gt "$limit" ]; then
        {
            echo "$marker"
            sed '/^<details>/,$d' "$report" | head -c "$limit" | sed '$d'
            printf '\n_The report is too long for a comment. The rest is in the job summary._\n'
        } > "$comment"
    fi

    existing=$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues/$PULL_REQUEST/comments" \
        --jq ".[] | select(.body | startswith(\"$marker\")) | .id" | head -n 1)

    if [ -n "$existing" ]; then
        gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$existing" --field "body=@$comment" > /dev/null \
            || echo "::warning::Could not update the puml comment. A pull request from a fork gets a read-only token."
    elif [ "$(jq '.files | length' "$json")" -gt 0 ]; then
        gh api --method POST "repos/$GITHUB_REPOSITORY/issues/$PULL_REQUEST/comments" --field "body=@$comment" > /dev/null \
            || echo "::warning::Could not comment on the pull request. A pull request from a fork gets a read-only token."
    fi
fi

if [ "$PUML_STRICT" = true ] && [ "$worsens" = true ]; then
    echo "::error::This pull request makes the architecture worse. The job summary says how."
    exit 1
fi

#!/usr/bin/env bash
# Runs puml-extract diff, writes the job summary and keeps one comment on the pull request up to date.
set -euo pipefail

marker='<!-- puml-diff -->'
limit=65000
directory="$RUNNER_TEMP/puml"
report="$directory/diff.md"
comment="$directory/comment.md"

if [ -z "$PUML_BASE" ]; then
    echo "::error::Set the base input: only pull_request events have a base to compare with."
    exit 1
fi

mkdir -p "$directory"

status=0
puml-extract diff "$PUML_DIRECTORY" "$PUML_BASE" --format markdown --strict > "$report" || status=$?

if [ "$status" -gt 1 ]; then
    exit "$status"
fi

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
    elif [ "$(puml-extract diff "$PUML_DIRECTORY" "$PUML_BASE" --format json | jq '.files | length')" -gt 0 ]; then
        gh api --method POST "repos/$GITHUB_REPOSITORY/issues/$PULL_REQUEST/comments" --field "body=@$comment" > /dev/null \
            || echo "::warning::Could not comment on the pull request. A pull request from a fork gets a read-only token."
    fi
fi

if [ "$PUML_STRICT" = true ] && [ "$worsens" = true ]; then
    echo "::error::This pull request makes the architecture worse. The job summary says how."
    exit 1
fi

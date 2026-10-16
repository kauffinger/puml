#!/usr/bin/env bash
# Downloads a puml-extract release for this runner, checks it against SHA256SUMS and puts it on the PATH.
set -euo pipefail

case "$RUNNER_OS/$RUNNER_ARCH" in
    Linux/X64) target=x86_64-unknown-linux-musl ;;
    Linux/ARM64) target=aarch64-unknown-linux-musl ;;
    macOS/X64) target=x86_64-apple-darwin ;;
    macOS/ARM64) target=aarch64-apple-darwin ;;
    *)
        echo "::error::puml-extract has no release for $RUNNER_OS on $RUNNER_ARCH"
        exit 1
        ;;
esac

name="puml-extract-$PUML_VERSION-$target"
directory="$RUNNER_TEMP/puml"
releases="https://github.com/kauffinger/puml/releases/download/$PUML_VERSION"

mkdir -p "$directory"
cd "$directory"
curl --fail --silent --show-error --location --remote-name "$releases/$name.tar.gz"
curl --fail --silent --show-error --location --remote-name "$releases/SHA256SUMS"

grep " $name.tar.gz\$" SHA256SUMS | shasum --algorithm 256 --check

tar --extract --gzip --file "$name.tar.gz"
echo "$directory/$name" >> "$GITHUB_PATH"

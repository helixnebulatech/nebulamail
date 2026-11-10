#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only
#
# The body of a release: what is in it and how to check it.
#
#   scripts/release/notes.sh 1.2.3 helixnebulatech/nebulamail
set -eu

version=${1:?usage: notes.sh <version> <owner/repo>}
repo=${2:?usage: notes.sh <version> <owner/repo>}
nonce="nebula-mail-$(printf %s "$version" | tr . -)"
identity="https://github.com/$repo/.github/workflows/release.yml@refs/tags/v$version"

cat <<NOTES
## Files

- \`nebula-mail-static-$version.zip\`: the static web app, served by Stalwart itself (no server of ours). Built with \`NEBULA_STYLE_NONCE=$nonce\` and the default settings, so it can be rebuilt byte for byte from this tag.
- \`nebula-bridge-$version-x86_64-linux-musl.tar.gz\`, \`nebula-bridge-$version-aarch64-linux-musl.tar.gz\`: the bridge, a static binary for any Linux of that architecture, with \`bridge.example.toml\` and the license.
- \`*.cdx.json\`: software bills of materials (CycloneDX), from the lockfiles (\`bun.lock\`, \`bridge/Cargo.lock\`).
- \`SHA256SUMS\`: SHA-256 of every file above.
- \`*.sigstore.json\`: keyless Sigstore signature of each file (and of \`SHA256SUMS\`), made by the release workflow of this repository for this tag.

## Check before installing

With [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) 2.4 or later:

\`\`\`sh
cosign verify-blob SHA256SUMS \\
  --bundle SHA256SUMS.sigstore.json \\
  --certificate-identity "$identity" \\
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum --check --ignore-missing SHA256SUMS
\`\`\`

The first command proves \`SHA256SUMS\` was made by this repository's release workflow for \`v$version\` (recorded in the public Rekor log); the second, that the files you downloaded are the ones listed. Each file can also be checked on its own with its \`.sigstore.json\` the same way.

Build provenance (SLSA), with the [GitHub CLI](https://cli.github.com/):

\`\`\`sh
gh attestation verify nebula-mail-static-$version.zip --repo $repo
\`\`\`
NOTES

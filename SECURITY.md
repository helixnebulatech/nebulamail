# Security policy

Security is a priority for Nebula Mail. Thank you for helping keep its users safe.

_En español: informa en privado (Security Advisory de GitHub o security@helixnebula.tech), nunca en un issue público. Respondemos en español o en inglés._

## Reporting a vulnerability

**Please do not open a public issue, discussion or pull request.** Report privately through one of:

1. **GitHub Security Advisories** (preferred): the "Report a vulnerability" button in the [Security tab](https://github.com/helixnebulatech/nebulamail/security/advisories/new) of `helixnebulatech/nebulamail`.
2. **Email**: [security@helixnebula.tech](mailto:security@helixnebula.tech).

Please include:

- the affected component (web app, image proxy, bridge, or a shared package such as `net-guard` or `mail-render`) and the version or commit;
- steps to reproduce (a minimal email, URL or request is ideal) and what an attacker gains;
- whether the issue is already public or known to others.

What to expect:

- an acknowledgement within **3 working days** and a first assessment within **7 days**;
- updates at least every 14 days until it is fixed;
- a fix and a published advisory (with a CVE when it applies), normally within **90 days**; we agree the disclosure date with you, earlier if it is being exploited;
- credit in the advisory if you want it.

Machine-readable contacts: `/.well-known/security.txt` on any instance of the web app ([RFC 9116](https://www.rfc-editor.org/rfc/rfc9116)).

## Supported versions

Nebula Mail has no stable release yet. Until 1.0, only the latest commit of the `main` branch is supported: fixes land there and are not backported. After 1.0, the latest minor release of each supported major version will get security fixes; this table will say which.

| Version          | Supported |
| ---------------- | --------- |
| `main` (pre-1.0) | Yes       |
| Anything older   | No        |

## Verifying a release

Releases are built by `.github/workflows/release.yml` from a `v<version>` tag. Every file (the static zip, the bridge binaries, the SBOMs and `SHA256SUMS`) has a keyless [Sigstore](https://www.sigstore.dev/) signature (`<file>.sigstore.json`) made by that workflow, and GitHub keeps their build provenance (SLSA). Check them before installing:

```sh
V=1.2.3
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/helixnebulatech/nebulamail/.github/workflows/release.yml@refs/tags/v$V" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify "nebula-mail-static-$V.zip" --repo helixnebulatech/nebulamail
```

A signature by any other identity (another repository, workflow or tag) is not ours. The static zip can also be rebuilt byte for byte from the tag (the release notes give the build settings) and compared with `SHA256SUMS`.

## Scope

In scope, everything in this repository:

- **Web app** (`apps/web`): session and token handling (OAuth, cookies, CSRF), authentication bypasses, XSS or CSP bypasses, and above all **escapes from the email sandbox**: script execution, network requests with remote images off, reading other emails or the session.
- **Image proxy** (`packages/image-proxy`): **SSRF** (reaching private or reserved addresses, other ports or schemes, DNS rebinding, redirects), use as an open proxy (downloading what is not in a signed ticket), leaks of the user's IP or of tickets, cache poisoning, the preload hook.
- **Bridge** (`bridge/`): SSRF through IMAP/SMTP/DAV hosts or autoconfiguration, credential storage and encryption, TLS downgrades, cross-account access.
- Shared packages: `net-guard` (TypeScript and Rust), `mail-render` (sanitizer pipeline), `jmap-client`.
- The deployment examples (`docker/`, `wrangler.example.toml`, `vercel.json`) when they lead to an insecure setup.

Out of scope:

- **Bugs in Stalwart itself** go to Stalwart, not here: [support.stalw.art](https://support.stalw.art). Stalwart does not accept AI-generated reports; reproduce and write it yourself.
- Bugs in other third-party code we only use (DOMPurify, Proton's sanitizer, undici…): report them upstream; tell us too if Nebula Mail is affected.
- Missing hardening without a concrete impact, self-XSS, clickjacking of pages with no state-changing action, rate limits that are merely generous, reports from automated scanners without a working proof, social engineering, and denial of service by sheer volume.
- Instances you do not run. Test against your own deployment (`docker/compose.yaml` starts a complete one).

## Safe harbour

We will not take legal action against, or ask anyone to take action against, research that:

- is done in good faith to find and report vulnerabilities to us;
- uses only your own accounts and instances, or accounts you have the owner's permission to test;
- does not access, change or keep other people's data beyond the minimum needed to show the issue, and deletes it afterwards;
- does not degrade the service for others (no volume-based denial of service, no spam);
- gives us reasonable time to fix the issue before anything is disclosed.

If in doubt, ask first at security@helixnebula.tech. If a third party takes action against you for research that followed this policy, we will make it known that it was authorised by us.

## Reports written with AI

You may use AI tools to find or analyse an issue, but a person must reproduce it, check that the impact is real, and write or review the report. Unverified AI-generated reports will be closed. (Stalwart does not accept AI-generated reports at all.)

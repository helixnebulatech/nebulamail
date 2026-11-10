# lettre 0.11.23, patched

Copy of [lettre](https://github.com/lettre/lettre) **0.11.23** as published on
crates.io (upstream commit `eb6136a06f1aba9432f73ef8f02a06aa0afc8f9c`), used
through `[patch.crates-io]` in `bridge/Cargo.toml`. Licence: MIT (`LICENSE`,
kept as is). Removed from the published package: `.cargo-ok`,
`.cargo_vcs_info.json`, `.github/`, `Cargo.lock`, `.gitignore`.

Every change is marked with a `PATCH (nebulamail, PATCHES.md)` comment.

**Drop this folder** (and the `[patch.crates-io]` entry) as soon as a lettre
release fixes the STARTTLS injection: then bump `lettre` in
`bridge/Cargo.toml` and check that `crates/smtp/tests/starttls_injection.rs`
still passes.

## 1. STARTTLS response injection (security)

`AsyncSmtpConnection::starttls` (`src/transport/smtp/client/async_connection.rs`)
and `SmtpConnection::starttls` (`client/connection.rs`) read the `220` reply
to `STARTTLS` through a `BufReader`, then upgrade the stream *under* that
`BufReader` (`self.stream.get_mut().upgrade_tls(..)`). Anything the
`BufReader` had already read after the `220` was sent in cleartext, but it
stays in the buffer and is later parsed as if it had come over TLS.

A man in the middle can therefore answer `STARTTLS` with
`220 ...\r\n` + forged replies in one packet; the forged replies answer
EHLO, AUTH, MAIL, RCPT, DATA…, and the real replies (over TLS) are read
late, one or more commands behind, or never.
`crates/smtp/tests/starttls_injection.rs` shows it: the unpatched crate
returns a forged `250 ... queued as FORGED` although the real server refused
the recipient. Same class as CVE-2011-0411 (Postfix) and CVE-2020-14954
(Mutt).

**Fix:** after the `220`, if the read buffer is not empty, abort (close the
connection, no `QUIT`) with a `Response` error, like Postfix. Our
`nebula-smtp` maps it to `SmtpError::Protocol { stage: StartTls }`.

Reported privately to the lettre maintainers on 2026-10-27 (GitHub private
vulnerability reporting, as their `SECURITY.md` on `master` asks;
GHSA-q3jf-wmp2-47pg), with a PoC
and the proposed fix. Not public yet: don't describe it in public issues or
commit messages, and don't make this repo public, until they publish a fix
(or 90 days pass).

## 2. Zeroize the password copies

`Credentials` keeps the password in a plain `String`, cloned for every AUTH
step, and `Auth` keeps the encoded SASL response. Now:

- `Drop for Credentials` wipes user name and password (`zeroize`);
- `Drop for Auth` wipes the SASL response, and its `Display` wipes the base64
  copy;
- `command()` wipes the command line it wrote (an `AUTH PLAIN` line has the
  password in base64).

Best effort: `format!` and `String` growth can still leave copies in freed
memory. New dependency: `zeroize` (already in our tree).

## 3. Build settings

- `Cargo.toml` (the normalized one cargo reads): `zeroize` dependency, and
  `dead_code` lints allowed (a path dependency gets no `--cap-lints`, so the
  dead code of lettre with our features warned in every build).
  `Cargo.toml.orig` has the same `zeroize` line, for the upstream diff.
- Not a workspace member (`exclude = ["vendor"]`): our clippy rules, `fmt`
  and `forbid(unsafe_code)` apply only to our crates. lettre itself has
  `#![forbid(unsafe_code)]` (`src/lib.rs`).

## 4. EHLO line with only spaces panics

`ServerInfo::from_response` (`src/transport/smtp/extension.rs`) did
`line.split_whitespace().next().unwrap()` on every EHLO line that is not
empty. A line of only spaces (`250-  `) has no word, so a server (or, before
STARTTLS, anyone in the middle) could make the client panic. The async and
the sync connection both use this function.

**Fix:** `let Some(keyword) = split.next() else { continue; };`, as
`ServerExtensions::from_ehlo_lines` in `nebula-smtp` already does. Unit test
`test_serverinfo_line_of_only_spaces`; `crates/smtp/src/op/connect.rs` has
`ehlo_line_of_only_spaces_does_not_panic`. No other `unwrap`/`expect` in
`src/transport/smtp` depends on what the server sends (`util.rs` and `Auth`
only see our own values; `first_word()` is already safe).

Not a secret: it can go upstream as a normal issue or PR, sent by a person
(this repo stays private because of §1).

Tests: lettre's own unit tests pass with our features
(`cargo test --manifest-path vendor/lettre/Cargo.toml --lib
--no-default-features --features smtp-transport,tokio1,tokio1-rustls,aws-lc-rs,rustls-platform-verifier,builder,pool`;
delete the `Cargo.lock` it creates).

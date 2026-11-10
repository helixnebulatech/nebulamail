<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The "Verified sender" badge: what it means and when it's wrong

Nebula Mail can show a green **Verified** badge next to the sender of an email. It always shows a **Not verified** warning when the sender's checks fail. This page explains what the badge means, how the app decides, when it can be wrong, and what to set up before you turn it on.

**Default: off.** Without the badge the app only shows warnings, and that is safe with any mail server.

## What "Verified" means

When your mail server receives a message, it checks it (SPF, DKIM, DMARC) and writes the results in an `Authentication-Results` header ([RFC 8601](https://www.rfc-editor.org/rfc/rfc8601)). The app reads that header and shows "Verified" only when **DMARC passed for the domain in the `From` address**. That means the domain shown really sent the message.

Why DMARC and not just DKIM or SPF: only DMARC checks that the passing DKIM signature or SPF domain _matches_ the `From` domain. An attacker can send `From: [EMAIL]` signed by their own domain `evil.example`: DKIM and SPF pass (for `evil.example`), DMARC doesn't. So "DKIM pass, SPF pass" without DMARC gives no badge.

"Verified" does **not** mean:

- **The sender is honest.** Look-alike domains (`paypa1.com`, `micros0ft-support.com`) pass DMARC if their owner set it up. So does any free account (`[EMAIL]` is verified for `gmail.com`) and any hacked account.
- **The display name is right.** `"PayPal" <[EMAIL]>` is verified for `other.example`, not for PayPal. The badge's tooltip names the domain that was checked.
- **The links and attachments are safe.**

International and look-alike sender domains get a separate badge that shows how the domain is really written. It appears even next to "Verified".

## How the app decides

Code: [`apps/web/src/mail/auth-results.ts`](../apps/web/src/mail/auth-results.ts) (tests in `auth-results.test.ts` next to it). The setting: [`apps/web/src/mail/use-verified-badge.ts`](../apps/web/src/mail/use-verified-badge.ts).

1. **Only the topmost `Authentication-Results` header.** Each server adds its header on top. The ones below can come from anyone, including the sender.
2. **Only if your server wrote it.** The header's authserv-id (its first word) must equal the hostname of the JMAP API URL the app talks to (case-insensitive). Otherwise the header is ignored: no badge and no warning.
3. **The verdict:**
   - **fail** if any `dmarc=fail`, or if DKIM or SPF failed (`fail`, `softfail` or `permerror`) and neither of them passed;
   - **pass** if a `dmarc=pass` has a `header.from` that is exactly the domain of the email's only `From` address (compared lowercase, without a final dot, in ASCII/IDNA form). A pass for another domain (even a parent or subdomain), or without `header.from`, doesn't count;
   - **no verdict** in every other case, and when the header can't be parsed (for example an unclosed quote or comment). No badge is better than a wrong one.

   Comments in the header are ignored, so `spf=fail (dmarc=pass)` is a fail. Text inside quoted strings is never read as a result.

4. **What is shown:** "fail" always; "pass" only if the badge is turned on.

## When the badge can be wrong

| Case | What happens | What to do |
| --- | --- | --- |
| The server only adds its header to mail from other servers (Stalwart's default: port 25 only) | A user with an account on your server sends a message with a fake header that says `dmarc=pass`, written with your server's name. The server adds nothing on top, so the app reads the fake one: **false "Verified"** | Make the server add its header to _all_ mail (below) before you turn the badge on |
| Mail that arrived before you changed that setting | It still has only the headers it arrived with, maybe fake ones: **false "Verified"** possible | Accept it, or keep the badge off. Old forged mail can't be told apart |
| Mail imported into a mailbox (IMAP `APPEND`, JMAP `Email/import`, migration tools, backups) | The server doesn't check it and keeps its old headers. If they name a server with the same hostname (e.g. your old server during a migration), the app trusts them: **false "Verified"** possible | Same as above |
| Other mail servers, or the bridge (below) | The app can't know if the server adds its header to all mail | Check your server's docs and run the test below |
| The hostname in the header isn't the JMAP API hostname | The header is ignored: no badge and no warning | Safe, but nothing is shown |
| More than one `From` address | No badge (DMARC checks one domain). Failures are still shown | — |
| Mailing lists, forwarding, gateways in front of your server | They change the message or the sending IP, so legitimate mail can fail: **false "Not verified"** | Known limitation: the app doesn't use ARC |

Also:

- **No warning doesn't mean safe.** On a server that doesn't check all mail, a fake header can also hide a warning.
- A forged _failure_ only makes an email look worse, so warnings are always shown, badge on or off.

## Setting it up

### 1. Make your server add its header to all mail

**Stalwart:** set `addAuthResultsHeader` to `true` for every path, with no conditions (it's in the MTA's DATA-stage settings, the `MtaStageData` object of the management API). By default it only applies to mail arriving on port 25. For the development server, `bun run dev:setup` ([`docker/setup.ts`](../docker/setup.ts)) does this for you. Then the top header of local mail is the server's own, e.g. `localhost; none`.

**Behind the bridge (other IMAP servers):** the bridge passes the stored headers through unchanged; it doesn't check mail or add its own header. So:

- the header must be written by the mail server in front of your IMAP server, on all mail, including mail from its own users;
- its authserv-id must equal the hostname the bridge's JMAP API is published on. Otherwise the app ignores the header and shows nothing. That's safe, but no badge and no warnings.

### 2. Test it

As a user of your server, send yourself a message that carries a fake header, both from the app (JMAP) and through SMTP submission (ports 465/587). Use your server's own name as the authserv-id:

```text
Authentication-Results: your.mail.host; dkim=pass header.d=paypal.com; dmarc=pass header.from=paypal.com
```

Open the received message's source. The **top** `Authentication-Results` must be your server's (e.g. `your.mail.host; none`), with the fake one below it. If the fake one is on top, don't turn the badge on.

### 3. Turn it on

Choose one:

- **Let each user decide:** Settings → General → Reading → "Show “Verified sender” badge" (off by default). The switch has a "What are the risks?" link. The choice is saved on the device and synced to the user's other devices when preference sync is available.
- **Decide for everyone:** set `VERIFIED_SENDER_BADGE` in the web server's environment ([`apps/web/.env.schema`](../apps/web/.env.schema)):
  - `true`: the badge is on for everyone;
  - `false`: the badge is off for everyone;
  - empty (default): each user decides.

  When forced, the switch is shown locked with "Set by your administrator."

Users can't see how your server is set up. If you've done steps 1 and 2, forcing `VERIFIED_SENDER_BADGE=true` is better than asking each user to turn it on.

The static build ([static.md](static.md)) has no web server, so `VERIFIED_SENDER_BADGE` doesn't apply there: each user decides in Settings.

Remember: mail received before step 1, and imported mail, stay unprotected (see the table).

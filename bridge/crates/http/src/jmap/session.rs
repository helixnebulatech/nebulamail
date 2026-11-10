// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `GET /.well-known/jmap`: the session object (RFC 8620 §2), built from
//! the configuration and the account's capabilities.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use nebula_jmap_proto::session::{
    Account, BridgeSessionCapability, DownloadUrl, EventSourceUrl, Session, SessionCapabilities,
    UploadUrl,
};
use nebula_jmap_proto::{Capability, State as JmapState};
use sha2::{Digest, Sha256};

use crate::backend::{AccountContext, Backend};
use crate::error::HttpError;
use crate::jmap::{authenticate, internal};
use crate::state::Shared;

/// Paths of the JMAP endpoints (the router uses the same ones).
pub(crate) const API_PATH: &str = "/jmap/api";
pub(crate) const UPLOAD_PATH: &str = "/jmap/upload/{accountId}";
pub(crate) const DOWNLOAD_PATH: &str = "/jmap/download/{accountId}/{blobId}/{name}?accept={type}";
pub(crate) const EVENT_SOURCE_PATH: &str =
    "/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}";

/// The session of an account. Its `state` is a hash of everything else,
/// so it changes exactly when the session does.
pub(crate) async fn build_session<B: Backend>(
    shared: &Shared<B>,
    account: &AccountContext,
) -> Result<Session, HttpError> {
    let capabilities = shared.backend.account_capabilities(account).await?;
    let url = shared.config.public_url();
    let template = |what: &'static str| HttpError::Corrupt(what);
    // One account: it is the primary one for everything it has.
    let primary_accounts: BTreeMap<Capability, _> = capabilities
        .supported()
        .into_iter()
        .map(|capability| (capability, account.jmap_id.clone()))
        .collect();
    let mut session_capabilities =
        SessionCapabilities::for_account(shared.config.jmap.core().clone(), &capabilities);
    // Tells the web app it talks to the bridge, and whether
    // scheduled mail can go out after sign-out.
    session_capabilities.bridge = Some(BridgeSessionCapability {
        send_when_signed_out: shared.config.signed_out_sending.is_on(),
    });
    // Web Push signed with the bridge's VAPID key.
    session_capabilities.web_push_vapid = shared.backend.web_push_key();
    let mut session = Session {
        capabilities: session_capabilities,
        accounts: [(
            account.jmap_id.clone(),
            Account {
                name: account.email.clone(),
                is_personal: true,
                is_read_only: false,
                account_capabilities: capabilities,
            },
        )]
        .into(),
        primary_accounts,
        username: account.login.clone(),
        api_url: url.join(API_PATH),
        download_url: DownloadUrl::new(url.join(DOWNLOAD_PATH))
            .map_err(|_| template("download URL"))?,
        upload_url: UploadUrl::new(url.join(UPLOAD_PATH)).map_err(|_| template("upload URL"))?,
        event_source_url: EventSourceUrl::new(url.join(EVENT_SOURCE_PATH))
            .map_err(|_| template("event source URL"))?,
        state: JmapState::new(""),
    };
    // Calendars and address books others share with the user: their
    // accounts, never primary (RFC 9670).
    for shared_account in shared.backend.shared_accounts(account).await? {
        session.accounts.insert(
            shared_account.id,
            Account {
                name: shared_account.name,
                is_personal: false,
                is_read_only: false,
                account_capabilities: shared_account.capabilities,
            },
        );
    }
    session.state = state_of(&session)?;
    Ok(session)
}

/// The first 64 bits of the SHA-256 of the session JSON, in hex.
fn state_of(session: &Session) -> Result<JmapState, HttpError> {
    let json = serde_json::to_vec(session).map_err(|_| HttpError::Corrupt("session"))?;
    let digest = Sha256::digest(&json);
    let mut state = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(state, "{byte:02x}");
    }
    Ok(JmapState::new(state))
}

/// `GET /.well-known/jmap`.
pub(crate) async fn session<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    request: Request,
) -> Response {
    let account = match authenticate(&shared, request.headers()).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match build_session(&shared, &account).await {
        Ok(session) => Json(session).into_response(),
        Err(error) => internal(&error),
    }
}

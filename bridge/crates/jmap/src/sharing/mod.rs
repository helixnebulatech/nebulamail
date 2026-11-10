// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharing calendars and address books through the bridge
//! the way Stalwart's JMAP does it:
//!
//! - The user's own collections have `shareWith` and `myRights.mayShare`
//!   where the DAV server lets them share ([`share_with`]).
//! - What others share with the user is in those people's accounts
//!   ([`accounts`]), and the calls on them only see those collections
//!   ([`scope`]).

pub(crate) mod accounts;
pub(crate) mod scope;
pub(crate) mod share_with;

pub(crate) use scope::Scope;

// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One file per operation.

pub(crate) mod append;
pub(crate) mod capability;
pub(crate) mod close;
pub(crate) mod connect;
pub(crate) mod copy_move;
pub(crate) mod fetch;
pub(crate) mod idle;
pub(crate) mod list;
pub(crate) mod login;
pub(crate) mod mailbox;
pub(crate) mod notify;
pub(crate) mod quota;
pub(crate) mod search;
pub(crate) mod select;
pub(crate) mod store;

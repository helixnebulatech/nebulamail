// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Pure translation from IMAP to the bridge's model: no I/O, unit-tested.

pub(crate) mod fetch;
pub(crate) mod header_form;
pub(crate) mod headers;
pub(crate) mod hierarchy;
pub(crate) mod keywords;
pub(crate) mod layout;
pub(crate) mod parsed;
pub(crate) mod preview;
pub(crate) mod report;
pub(crate) mod role;
pub(crate) mod text;
pub(crate) mod transfer;

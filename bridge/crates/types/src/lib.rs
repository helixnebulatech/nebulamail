// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Shared newtypes for the Nebula Mail bridge.
//!
//! Values from outside are parsed into these types once, at the edge
//! ("parse, don't validate"). Code that receives an [`Id`] can rely on it
//! being valid without checking again.

#![forbid(unsafe_code)]

// The JMAP `Id` lives in `nebula-jmap-proto` (MIT OR Apache-2.0): a
// permissive crate can't depend on this AGPL one, the other way round is fine.
pub use nebula_jmap_proto::types::{Id, IdError};

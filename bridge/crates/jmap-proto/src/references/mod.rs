// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! References inside a request: to earlier results (`#ids`, RFC 8620 §3.7)
//! and to objects created earlier (`#creationId`, RFC 8620 §5.3).

mod budget;
mod creation;
mod jsptr;
mod result;

pub use budget::{OverBudget, ReferenceBudget};
pub use creation::{CreatedIds, IdOrRef, UnknownCreationId};
pub use jsptr::{JsonPointer, PointerError};
pub use result::{ResultReference, resolve_result_references};

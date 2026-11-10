// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Pure translation between DAV resources and JMAP objects: no I/O.

pub mod alarm;
mod apple_groups;
pub mod calendar;
pub mod convert;
pub mod counter;
pub mod defaults;
pub mod edit;
pub mod freebusy;
pub(crate) mod index;
pub mod itip;
pub mod links;
pub mod sealed;
pub mod search;
mod task;

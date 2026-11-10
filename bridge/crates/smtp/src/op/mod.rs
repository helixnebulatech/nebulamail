// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One file per step of the dialogue: connect (with TLS), authenticate,
//! send.

mod auth;
mod connect;
mod send;

pub use send::Sent;

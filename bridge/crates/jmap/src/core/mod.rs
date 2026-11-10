// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The backend itself: configuration, the `Backend` implementation, the
//! method dispatcher, the per-request context, ids and push.

pub(crate) mod alerts;
pub(crate) mod backend;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod dispatch;
pub(crate) mod events;
pub(crate) mod ids;
pub(crate) mod text;

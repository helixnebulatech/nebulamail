// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMAP errors at the three levels of RFC 8620: the whole request
//! (§3.6.1), one method call (§3.6.2) and one object in `/set` (§5.3).

mod method;
mod request;
mod set;

pub use method::{MethodError, MethodErrorType};
pub use request::{
    LimitName, OVER_QUOTA_URI, PROBLEM_JSON_CONTENT_TYPE, RequestError, RequestErrorType,
};
pub use set::{SetError, SetErrorType};

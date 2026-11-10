// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The standard methods (RFC 8620 §5): arguments and responses shared by
//! every data type. Type-specific extras live with each object.

mod changes;
mod copy;
mod get;
mod import;
pub(crate) mod parse;
pub(crate) mod patch;
mod query;
mod query_changes;
mod set;
mod upload;

pub use changes::{ChangesArguments, ChangesResponse, MailboxChangesResponse};
pub use copy::{BlobCopyArguments, BlobCopyResponse, CopyArguments};
pub use get::{GetArguments, GetResponse};
pub use import::{EmailImport, EmailImportArguments, EmailImportResponse};
pub use patch::{Patch, PatchObject};
pub use query::{
    Collation, Comparator, Filter, FilterCondition, FilterOperator, MAX_FILTER_DEPTH,
    MAX_FILTER_LIST, MAX_FILTER_NODES, MAX_FILTER_TEXT, Operator, QueryArguments, QueryResponse,
    SortProperty,
};
pub use query_changes::{AddedItem, QueryChangesArguments, QueryChangesResponse};
pub use set::{SetArguments, SetResponse};
pub use upload::BlobUploadResponse;

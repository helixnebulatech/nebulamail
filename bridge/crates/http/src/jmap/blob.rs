// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Blob upload and download (RFC 8620 §6).

use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt as _};
use http::{HeaderValue, StatusCode, header};
use nebula_jmap_proto::error::LimitName;
use nebula_jmap_proto::method::BlobUploadResponse;
use nebula_jmap_proto::{BlobId, UnsignedInt};
use nebula_jmap_proto::{RequestError, RequestErrorType};

use crate::backend::{
    AccountContext, Backend, BackendError, BlobBody, MediaType, UploadBody, UploadBodyError,
    UploadError,
};
use crate::error::HttpError;
use crate::form::Form;
use crate::jmap::{
    authenticate, body_read_error, declared_length, internal, limit_error, not_found, request_error,
};
use crate::limits::DownloadPermit;
use crate::state::Shared;

/// Longest file name kept in `Content-Disposition`, in characters (see
/// [`safe_file_name`]).
const MAX_NAME_CHARS: usize = 255;

fn is_own_account(account: &AccountContext, requested: &str) -> bool {
    account.jmap_id.as_str() == requested
}

/// `POST /jmap/upload/{accountId}`.
pub(crate) async fn upload<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let account = match authenticate(&shared, &parts.headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    if !is_own_account(&account, &account_id) {
        return not_found();
    }
    let Some(_slot) = shared.upload_slots.try_acquire(account.id) else {
        return limit_error(
            LimitName::MaxConcurrentUpload,
            "Too many concurrent uploads for this account.",
        );
    };
    let media_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<MediaType>().ok())
        .unwrap_or_else(MediaType::octet_stream);
    let limit = shared.config.jmap.max_size_upload();
    let Ok(limit_bytes) = u64::try_from(limit) else {
        return internal(&HttpError::Corrupt("upload limit"));
    };
    // Streamed to the backend: never whole in memory here.
    let body = UploadBody::new(body, declared_length(&parts.headers), limit_bytes);
    let blob = match shared.backend.upload(&account, media_type, body).await {
        Ok(blob) => blob,
        Err(UploadError::Body(UploadBodyError::TooLarge)) => {
            return limit_error(
                LimitName::MaxSizeUpload,
                &format!("The upload is larger than {limit} bytes."),
            );
        }
        Err(UploadError::Body(UploadBodyError::Read)) => return body_read_error(),
        Err(UploadError::OverQuota) => {
            return request_error(&RequestError::new(
                RequestErrorType::OverQuota,
                "The account has no room for more uploads; try again later.",
            ));
        }
        Err(UploadError::Backend(error)) => return internal(&HttpError::from(error)),
    };
    let Ok(size) = UnsignedInt::try_from(blob.size) else {
        return internal(&HttpError::Corrupt("blob size"));
    };
    let body = BlobUploadResponse {
        account_id: account.jmap_id,
        blob_id: blob.blob_id,
        media_type: blob.media_type.as_str().to_owned(),
        size,
    };
    (StatusCode::CREATED, Json(body)).into_response()
}

/// `GET /jmap/download/{accountId}/{blobId}/{name}?accept={type}`.
pub(crate) async fn download<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    Path((account_id, blob_id, name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let account = match authenticate(&shared, request.headers()).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    if !is_own_account(&account, &account_id) {
        return not_found();
    }
    let Ok(blob_id) = blob_id.parse::<BlobId>() else {
        return not_found();
    };
    let accept = Form::parse(request.uri().query().unwrap_or_default().as_bytes())
        .ok()
        .and_then(|form| form.get("accept").and_then(|t| t.parse::<MediaType>().ok()));
    // Taken before the backend reads anything, and held by the body until
    // it is sent or dropped: a client that doesn't read keeps its slot.
    let Some(permit) = shared
        .downloads
        .acquire(account.id, shared.download_wait)
        .await
    else {
        return limit_error(
            LimitName::MaxConcurrentRequests,
            "Too many concurrent downloads; try again later.",
        );
    };
    let blob = match shared.backend.download(&account, &blob_id).await {
        Ok(Some(blob)) => blob,
        Ok(None) => return not_found(),
        Err(error) => return internal(&HttpError::from(error)),
    };
    let media_type = accept.unwrap_or(blob.media_type);
    let (size, chunks) = match blob.body {
        BlobBody::Bytes(bytes) => {
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            (size, futures::stream::once(async { Ok(bytes) }).boxed())
        }
        BlobBody::Stream { size, chunks } => (size, chunks),
    };
    let body = Body::from_stream(WithPermit {
        chunks,
        _permit: permit,
    });
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(media_type.as_str()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(&name));
    // RFC 8620 §6.2 recommends `private, immutable, max-age=31536000`
    // (a blob never changes). Not followed, for privacy: mail and its
    // attachments would stay in the browser's disk cache, readable after
    // signing out and on a shared computer.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // Mail content is hostile: if a browser ever renders it from this
    // origin, it can't run scripts or load anything.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

/// A download's chunks, holding its slots until they are all sent or the
/// body is dropped.
struct WithPermit {
    chunks: BoxStream<'static, Result<Bytes, BackendError>>,
    _permit: DownloadPermit,
}

impl Stream for WithPermit {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut()
            .chunks
            .poll_next_unpin(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map_err(std::io::Error::other)))
    }
}

/// Characters removed from a file name, the only change made to the
/// `name` the client asked for (RFC 8620 §6.2: the server MUST return it
/// as the file name). Each removal has a security reason
///
///
/// - controls (C0, DEL, C1): no file system wants them, and CR/LF or ESC
///   in a name end up in terminals and logs;
/// - invisible and bidirectional formatting characters, which disguise an
///   extension (`invoice\u{202E}fdp.exe` shows as `invoiceexe.pdf`);
/// - `/` and `\`, so no client saves outside its download folder.
///
/// Everything else, `% ; : " * ? < > |` included, goes in `filename*`
/// percent-encoded (RFC 8187 §3.2), exactly as given.
fn is_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(c, '/' | '\\')
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// Longest extension kept when a name is cut, in characters.
const MAX_EXTENSION_CHARS: usize = 16;

/// The file name of a download: [`is_unsafe`] characters dropped and
/// leading dots too (a download is never a hidden or configuration file,
/// `.bashrc`, `.htaccess`); at most [`MAX_NAME_CHARS`], keeping a short
/// extension when it is cut (no common file system keeps longer names:
/// ext4 has 255 bytes, NTFS 255 UTF-16 units; it also bounds the header);
/// `download` if nothing (but spaces) is left.
pub(crate) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !is_unsafe(*c)).collect();
    let cleaned = cleaned.trim_start_matches('.');
    if cleaned.trim().is_empty() {
        return "download".to_owned();
    }
    if cleaned.chars().count() <= MAX_NAME_CHARS {
        return cleaned.to_owned();
    }
    let extension = cleaned
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.chars().count() <= MAX_EXTENSION_CHARS);
    match extension {
        Some(ext) => {
            let keep = MAX_NAME_CHARS - ext.chars().count() - 1;
            let stem: String = cleaned.chars().take(keep).collect();
            format!("{stem}.{ext}")
        }
        None => cleaned.chars().take(MAX_NAME_CHARS).collect(),
    }
}

/// The ASCII `filename` for clients without RFC 8187 (RFC 6266 §4.3,
/// Appendix D): non-ASCII characters, and `"`, `\` and `%` (which some
/// clients unescape), become `_`.
fn ascii_fallback(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | '\\' | '%' => '_',
            c if c.is_ascii() => c,
            _ => '_',
        })
        .collect()
}

/// `attachment` with an ASCII `filename` and the exact UTF-8 `filename*`
/// (RFC 6266 §4.1, §4.3; RFC 8187 §3.2.1: `attr-char` stays, every other
/// byte is `%XX`).
fn content_disposition(name: &str) -> HeaderValue {
    let name = safe_file_name(name);
    let ascii = ascii_fallback(&name);
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_cleaned() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_file_name("a\\b"), "ab");
        assert_eq!(safe_file_name("invoice\u{202E}fdp.exe"), "invoicefdp.exe");
        assert_eq!(safe_file_name("a\u{200B}b\u{2066}c\u{FEFF}"), "abc");
        assert_eq!(safe_file_name("a\r\nb\u{1b}c\u{85}d"), "abcd");
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name("..."), "download");
        assert_eq!(safe_file_name(" . "), " . ");
        assert_eq!(safe_file_name("   "), "download");
        assert_eq!(safe_file_name(""), "download");
    }

    /// RFC 8620 §6.2: the name is the file name, exactly, with what has no
    /// security reason to go.
    #[test]
    fn file_names_are_kept_exactly() {
        for name in [
            "100% done; final: \"v2\" *?<>|.txt",
            "  spaced  .txt ",
            "Factura año 2026 — 日本語.pdf",
            "x".repeat(MAX_NAME_CHARS).as_str(),
        ] {
            assert_eq!(safe_file_name(name), name);
        }
    }

    /// Too long: cut to the limit, the extension kept.
    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.pdf", "ñ".repeat(500));
        let cut = safe_file_name(&long);
        assert_eq!(cut.chars().count(), MAX_NAME_CHARS);
        assert!(cut.ends_with("ñ.pdf"));
        let no_ext = "x".repeat(500);
        assert_eq!(safe_file_name(&no_ext).len(), MAX_NAME_CHARS);
        let odd_ext = format!("a.{}", "y".repeat(400));
        assert_eq!(safe_file_name(&odd_ext).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn content_disposition_has_both_forms() {
        assert_eq!(
            content_disposition("Factura año.pdf").to_str().unwrap(),
            "attachment; filename=\"Factura a_o.pdf\"; filename*=UTF-8''Factura%20a%C3%B1o.pdf"
        );
        assert_eq!(
            content_disposition("50% \"a\";b.txt").to_str().unwrap(),
            "attachment; filename=\"50_ _a_;b.txt\"; filename*=UTF-8''50%25%20%22a%22%3Bb.txt"
        );
    }
}

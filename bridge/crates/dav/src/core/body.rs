// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reading a response body with every limit: the size
//! (announced and real, compressed and decompressed), the time without
//! data and the request's deadline.
//!
//! We send `Accept-Encoding: identity`; a server that compresses anyway
//! (gzip or deflate) is decoded as the bytes arrive, into a buffer that
//! refuses to grow past the limit, so a gzip bomb stops at the limit.

use std::io::{self, Write as _};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use flate2::write::{GzDecoder, ZlibDecoder};
use http::HeaderMap;
use http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
use http_body_util::BodyExt as _;
use hyper::body::Incoming;
use tokio::time::Instant;

use crate::DavError;

/// A `Vec` that refuses to grow past `limit` bytes.
struct Capped {
    buffer: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl io::Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buffer.len().saturating_add(data.len()) > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("body over its limit"));
        }
        self.buffer.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum Decoder {
    Identity(BytesMut),
    Gzip(GzDecoder<Capped>),
    Deflate(ZlibDecoder<Capped>),
}

impl Decoder {
    fn for_headers(headers: &HeaderMap, limit: usize) -> Result<Self, DavError> {
        let mut encodings = headers.get_all(CONTENT_ENCODING).iter();
        let first = encodings.next();
        if encodings.next().is_some() {
            return Err(DavError::ContentEncoding);
        }
        let capped = || Capped {
            buffer: Vec::new(),
            limit,
            exceeded: false,
        };
        let name = match first {
            None => return Ok(Self::Identity(BytesMut::new())),
            Some(value) => value
                .to_str()
                .map_err(|_| DavError::ContentEncoding)?
                .trim(),
        };
        if name.eq_ignore_ascii_case("identity") {
            Ok(Self::Identity(BytesMut::new()))
        } else if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
            Ok(Self::Gzip(GzDecoder::new(capped())))
        } else if name.eq_ignore_ascii_case("deflate") {
            Ok(Self::Deflate(ZlibDecoder::new(capped())))
        } else {
            // br, zstd, a list of codings…: refused, not guessed.
            Err(DavError::ContentEncoding)
        }
    }

    fn feed(&mut self, data: &[u8], limit: usize) -> Result<(), DavError> {
        let result = match self {
            Self::Identity(buffer) => {
                if buffer.len().saturating_add(data.len()) > limit {
                    return Err(DavError::TooLarge);
                }
                buffer.extend_from_slice(data);
                return Ok(());
            }
            Self::Gzip(decoder) => decoder
                .write_all(data)
                .map_err(|_| decoder.get_ref().exceeded),
            Self::Deflate(decoder) => decoder
                .write_all(data)
                .map_err(|_| decoder.get_ref().exceeded),
        };
        result.map_err(|exceeded| {
            if exceeded {
                DavError::TooLarge
            } else {
                DavError::Decompress
            }
        })
    }

    fn finish(self) -> Result<Bytes, DavError> {
        // `try_finish` flushes the rest, which may still hit the limit:
        // that is `TooLarge` (the status of an error answer is kept),
        // anything else a broken stream.
        let flushed = |result: io::Result<()>, capped: &Capped| {
            result.map_err(|_| {
                if capped.exceeded {
                    DavError::TooLarge
                } else {
                    DavError::Decompress
                }
            })
        };
        let capped = match self {
            Self::Identity(buffer) => return Ok(buffer.freeze()),
            Self::Gzip(mut decoder) => {
                let result = decoder.try_finish();
                flushed(result, decoder.get_ref())?;
                decoder.finish()
            }
            Self::Deflate(mut decoder) => {
                let result = decoder.try_finish();
                flushed(result, decoder.get_ref())?;
                decoder.finish()
            }
        };
        capped
            .map(|capped| Bytes::from(capped.buffer))
            .map_err(|_| DavError::Decompress)
    }
}

/// Reads the whole body: at most `limit` bytes on the wire and after
/// decoding, never more than `idle` without data, done by `deadline`.
pub(crate) async fn read_body(
    mut body: Incoming,
    headers: &HeaderMap,
    limit: usize,
    idle: Duration,
    deadline: Instant,
) -> Result<Bytes, DavError> {
    if let Some(length) = headers.get(CONTENT_LENGTH) {
        let length: u64 = length
            .to_str()
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .ok_or(DavError::Protocol("invalid Content-Length"))?;
        if usize::try_from(length).map_or(true, |length| length > limit) {
            return Err(DavError::TooLarge);
        }
    }
    let mut decoder = Decoder::for_headers(headers, limit)?;
    let mut wire = 0_usize;
    loop {
        let wait = (Instant::now() + idle).min(deadline);
        let frame = tokio::time::timeout_at(wait, body.frame())
            .await
            .map_err(|_| DavError::Timeout)?;
        let Some(frame) = frame else {
            break;
        };
        let frame = frame?;
        if let Ok(data) = frame.into_data() {
            wire = wire.saturating_add(data.len());
            if wire > limit {
                return Err(DavError::TooLarge);
            }
            decoder.feed(&data, limit)?;
        }
    }
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use http::HeaderMap;
    use http::header::CONTENT_ENCODING;

    use super::Decoder;
    use crate::DavError;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn headers(encoding: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, encoding.parse().unwrap());
        headers
    }

    #[test]
    fn decodes_gzip_within_the_limit() {
        let mut decoder = Decoder::for_headers(&headers("gzip"), 1000).unwrap();
        decoder.feed(&gzip(b"<multistatus/>"), 1000).unwrap();
        assert_eq!(&decoder.finish().unwrap()[..], b"<multistatus/>");
    }

    #[test]
    fn gzip_bomb_stops_at_the_limit() {
        let bomb = gzip(&vec![b'a'; 10 * 1024 * 1024]);
        assert!(bomb.len() < 20 * 1024);
        let mut decoder = Decoder::for_headers(&headers("GZIP"), 64 * 1024).unwrap();
        let result = bomb
            .chunks(512)
            .try_for_each(|chunk| decoder.feed(chunk, 64 * 1024));
        assert!(matches!(result, Err(DavError::TooLarge)));
    }

    #[test]
    fn unknown_or_stacked_encodings_are_refused() {
        for encoding in ["br", "zstd", "gzip, gzip", "compress"] {
            assert!(matches!(
                Decoder::for_headers(&headers(encoding), 10),
                Err(DavError::ContentEncoding)
            ));
        }
        let mut two = headers("gzip");
        two.append(CONTENT_ENCODING, "gzip".parse().unwrap());
        assert!(matches!(
            Decoder::for_headers(&two, 10),
            Err(DavError::ContentEncoding)
        ));
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// Whether the limit is reached while feeding or in the final flush,
    /// it is `TooLarge`, never `Decompress`.
    #[test]
    fn limit_reached_in_the_final_flush_is_too_large() {
        type Compress = fn(&[u8]) -> Vec<u8>;
        let compressors: [(&str, Compress); 2] = [("gzip", gzip), ("deflate", deflate)];
        for (encoding, compress) in compressors {
            for limit in 1000..1100 {
                let body = compress(&vec![b'a'; limit + 1]);
                let mut decoder = Decoder::for_headers(&headers(encoding), limit).unwrap();
                let result = decoder
                    .feed(&body, limit)
                    .and_then(|()| decoder.finish().map(|_| ()));
                assert!(
                    matches!(result, Err(DavError::TooLarge)),
                    "{encoding} {limit}: {result:?}"
                );
                // Exactly at the limit, it fits.
                let body = compress(&vec![b'a'; limit]);
                let mut decoder = Decoder::for_headers(&headers(encoding), limit).unwrap();
                decoder.feed(&body, limit).unwrap();
                assert_eq!(decoder.finish().unwrap().len(), limit);
            }
        }
    }

    #[test]
    fn garbage_is_not_gzip() {
        let mut decoder = Decoder::for_headers(&headers("gzip"), 1000).unwrap();
        let result = decoder.feed(b"definitely not gzip", 1000);
        assert!(matches!(result, Err(DavError::Decompress)) || decoder.finish().is_err());
    }
}

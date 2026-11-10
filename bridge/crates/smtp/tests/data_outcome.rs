// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a failure around `DATA` means, against a fake server over implicit
//! TLS: before the `354` nothing was sent (transient); after the message
//! was written, a timeout or a dropped connection leaves the outcome
//! unknown (`Stage::DataEnd`), never "try again".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod common;

use std::time::Duration;

use nebula_smtp::{
    CertificateCheck, Credentials, Envelope, OutgoingMessage, Security, SmtpClient, SmtpError,
    SmtpServer, Stage, Timeouts,
};
use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const DATA_TIMEOUT: Duration = Duration::from_millis(300);

/// What the fake server does around `DATA`.
#[derive(Clone, Copy)]
enum AtData {
    /// `451` to the `DATA` command.
    Refuse,
    /// Reads the message up to `.` and never answers.
    Silent,
    /// Reads the message up to `.` and hangs up.
    HangUp,
    /// Reads the message slowly (a pause every 64 KiB), but never stops,
    /// then accepts it.
    SlowReader,
    /// Reads 64 KiB of the message and then nothing more.
    StopsReading,
    /// Reads the whole message and answers `250` after this long.
    SlowReply(Duration),
}

/// How much the slow fake servers read between pauses.
const CHUNK: usize = 64 * 1024;

async fn server(at_data: AtData) -> (TcpStream, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let tls = common::acceptor().accept(socket).await.unwrap();
        let mut stream = BufReader::new(tls);
        let _ = stream.get_mut().write_all(b"220 fake ESMTP\r\n").await;
        let mut in_data = false;
        let mut read_in_data = 0_usize;
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            if in_data {
                let before = read_in_data / CHUNK;
                read_in_data += line.len();
                if read_in_data / CHUNK != before {
                    match at_data {
                        AtData::SlowReader => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        AtData::StopsReading => {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            return;
                        }
                        _ => {}
                    }
                }
                if line == ".\r\n" {
                    match at_data {
                        AtData::SlowReader => {
                            in_data = false;
                            if stream.get_mut().write_all(b"250 queued\r\n").await.is_err() {
                                return;
                            }
                        }
                        AtData::SlowReply(after) => {
                            tokio::time::sleep(after).await;
                            in_data = false;
                            if stream.get_mut().write_all(b"250 queued\r\n").await.is_err() {
                                return;
                            }
                        }
                        AtData::Silent => {
                            // Keep the connection open past the timeout.
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            return;
                        }
                        AtData::HangUp | AtData::Refuse | AtData::StopsReading => return,
                    }
                }
                continue;
            }
            let upper = line.to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") {
                b"250-fake\r\n250 AUTH PLAIN\r\n"
            } else if upper.starts_with("AUTH") {
                b"235 ok\r\n"
            } else if upper.starts_with("DATA") {
                if matches!(at_data, AtData::Refuse) {
                    b"451 4.3.0 try later\r\n"
                } else {
                    in_data = true;
                    b"354 go\r\n"
                }
            } else if upper.starts_with("QUIT") {
                b"221 bye\r\n"
            } else {
                b"250 ok\r\n"
            };
            if stream.get_mut().write_all(reply).await.is_err() {
                return;
            }
        }
    });
    (TcpStream::connect(address).await.unwrap(), task)
}

async fn send(stream: TcpStream) -> Result<(), SmtpError> {
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    send_with(
        stream,
        Timeouts {
            command: Duration::from_secs(5),
            data_start: DATA_TIMEOUT,
            data_block: DATA_TIMEOUT,
            data_end: DATA_TIMEOUT,
        },
        &message,
    )
    .await
}

async fn send_with(
    stream: TcpStream,
    timeouts: Timeouts,
    message: &OutgoingMessage,
) -> Result<(), SmtpError> {
    let server = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_timeouts(timeouts);
    let credentials = Credentials::new("alice", SecretString::from("secret")).unwrap();
    let envelope = Envelope::new(
        "alice@example.com".parse().unwrap(),
        vec!["bob@example.net".parse().unwrap()],
    )
    .unwrap();
    let mut session = SmtpClient::new(server)
        .unwrap()
        .connect(stream)
        .await?
        .authenticate(&credentials)
        .await?;
    session.send(&envelope, message).await.map(|_| ())
}

#[tokio::test]
async fn no_reply_after_the_message_is_an_unknown_outcome() {
    let (stream, _server) = server(AtData::Silent).await;
    let error = send(stream).await.unwrap_err();
    assert_eq!(
        error,
        SmtpError::Timeout {
            stage: Stage::DataEnd,
            after: DATA_TIMEOUT
        }
    );
    assert!(error.outcome_unknown());
    assert!(!error.is_transient());
}

#[tokio::test]
async fn hang_up_after_the_message_is_an_unknown_outcome() {
    let (stream, _server) = server(AtData::HangUp).await;
    let error = send(stream).await.unwrap_err();
    assert!(
        matches!(
            error,
            SmtpError::Network {
                stage: Stage::DataEnd
            } | SmtpError::Protocol {
                stage: Stage::DataEnd
            }
        ),
        "{error:?}"
    );
    assert!(error.outcome_unknown());
    assert!(!error.is_transient());
}

#[tokio::test]
async fn refused_data_command_is_transient() {
    let (stream, _server) = server(AtData::Refuse).await;
    let error = send(stream).await.unwrap_err();
    assert!(
        matches!(
            error,
            SmtpError::Rejected {
                stage: Stage::Data,
                permanent: false,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.is_transient());
    assert!(!error.outcome_unknown());
}

/// A message far bigger than the socket buffers (so the client's writes
/// wait for the server to read), in short lines.
fn big_message() -> OutgoingMessage {
    let mut data = b"Subject: big\r\n\r\n".to_vec();
    let line = [b'x'; 76];
    while data.len() < 24 * 1024 * 1024 {
        data.extend_from_slice(&line);
        data.extend_from_slice(b"\r\n");
    }
    OutgoingMessage::new(&data).unwrap()
}

/// The data timeouts of RFC 5321 (scaled down): a short block timeout, a
/// longer one for the final reply.
const BLOCK: Duration = Duration::from_millis(500);
const END: Duration = Duration::from_millis(1500);

fn data_timeouts() -> Timeouts {
    Timeouts {
        command: Duration::from_secs(5),
        data_start: Duration::from_secs(5),
        data_block: BLOCK,
        data_end: END,
    }
}

#[tokio::test]
async fn a_slow_transfer_that_keeps_moving_is_never_cut() {
    let (stream, _server) = server(AtData::SlowReader).await;
    let started = std::time::Instant::now();
    send_with(stream, data_timeouts(), &big_message())
        .await
        .unwrap();
    // It took far longer than any one timeout: there is no limit on the
    // whole transfer, only on a stall.
    assert!(started.elapsed() > END, "{:?}", started.elapsed());
}

#[tokio::test]
async fn a_transfer_that_stalls_is_cut_after_the_block_timeout() {
    let (stream, _server) = server(AtData::StopsReading).await;
    let message = big_message();
    let error = send_with(stream, data_timeouts(), &message)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SmtpError::Timeout {
            stage: Stage::DataEnd,
            after: BLOCK
        }
    );
    // Cut by the block timeout, not the reply's (the error says which).
    assert!(error.outcome_unknown());
}

#[tokio::test]
async fn the_final_reply_gets_the_longer_timeout() {
    // Longer than the block timeout, shorter than the reply's: accepted.
    let (stream, _server) = server(AtData::SlowReply(Duration::from_millis(1000))).await;
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    send_with(stream, data_timeouts(), &message).await.unwrap();
    // Longer than that: unknown outcome, after the reply's timeout.
    let (stream, _server) = server(AtData::SlowReply(Duration::from_secs(5))).await;
    let error = send_with(stream, data_timeouts(), &message)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SmtpError::Timeout {
            stage: Stage::DataEnd,
            after: END
        }
    );
}

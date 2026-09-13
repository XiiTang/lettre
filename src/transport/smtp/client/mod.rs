//! SMTP client
//!
//! `SmtpConnection` allows manually sending SMTP commands.
//!
//! ```rust,no_run
//! # use std::error::Error;
//!
//! # #[cfg(feature = "smtp-transport")]
//! # fn main() -> Result<(), Box<dyn Error>> {
//! use lettre::transport::smtp::{
//!     SMTP_PORT, client::SmtpConnection, commands::*, extension::ClientId,
//! };
//!
//! let hello = ClientId::Domain("my_hostname".to_owned());
//! let mut client = SmtpConnection::connect(&("localhost", SMTP_PORT), None, &hello, None, None)?;
//! client.command(Mail::new(Some("user@example.com".parse()?), vec![]))?;
//! client.command(Rcpt::new("user@example.org".parse()?, vec![]))?;
//! client.command(Data)?;
//! client.message("Test email".as_bytes())?;
//! client.command(Quit)?;
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "serde")]
use std::fmt::Debug;

#[cfg(any(feature = "tokio1", feature = "async-std1"))]
pub use self::async_connection::AsyncSmtpConnection;
#[cfg(any(feature = "tokio1", feature = "async-std1"))]
#[allow(deprecated)]
pub use self::async_net::AsyncNetworkStream;
#[cfg(feature = "tokio1")]
pub use self::async_net::AsyncTokioStream;
use self::net::NetworkStream;
#[cfg(any(feature = "native-tls", feature = "rustls", feature = "boring-tls"))]
pub(super) use self::tls::InnerTlsParameters;
#[cfg(any(feature = "native-tls", feature = "rustls", feature = "boring-tls"))]
pub use self::tls::TlsVersion;
pub use self::{
    connection::SmtpConnection,
    tls::{Certificate, CertificateStore, Identity, Tls, TlsParameters, TlsParametersBuilder},
};

#[cfg(any(feature = "tokio1", feature = "async-std1"))]
mod async_connection;
#[cfg(any(feature = "tokio1", feature = "async-std1"))]
mod async_net;
mod connection;
mod net;
mod tls;

/// Total bytes cap on an SMTP response (Postfix `smtp_response_limit`).
pub(super) const MAX_RESPONSE_BYTES: usize = 100_000;

/// Single-line byte cap (Postfix `line_length_limit`).
pub(super) const MAX_RESPONSE_LINE_BYTES: usize = 1000;

/// The codec used for transparency
#[derive(Debug)]
struct ClientCodec {
    status: CodecStatus,
}

impl ClientCodec {
    /// Creates a new client codec
    pub(crate) fn new() -> Self {
        Self {
            status: CodecStatus::StartOfNewLine,
        }
    }

    /// Adds transparency
    fn encode(&mut self, frame: &[u8], buf: &mut Vec<u8>) {
        for &b in frame {
            buf.push(b);
            match (b, self.status) {
                (b'\r', _) => {
                    self.status = CodecStatus::StartingNewLine;
                }
                (b'\n', CodecStatus::StartingNewLine) => {
                    self.status = CodecStatus::StartOfNewLine;
                }
                (_, CodecStatus::StartingNewLine) => {
                    self.status = CodecStatus::MiddleOfLine;
                }
                (b'.', CodecStatus::StartOfNewLine) => {
                    self.status = CodecStatus::MiddleOfLine;
                    buf.push(b'.');
                }
                (_, CodecStatus::StartOfNewLine) => {
                    self.status = CodecStatus::MiddleOfLine;
                }
                _ => {}
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
#[allow(clippy::enum_variant_names)]
enum CodecStatus {
    /// We are past the first character of the current line
    MiddleOfLine,
    /// We just read a `\r` character
    StartingNewLine,
    /// We are at the start of a new line
    StartOfNewLine,
}

/// CRLF, line, transfer-mode and total-byte validation shared by DATA and BDAT.
#[derive(Clone, Copy, Debug)]
pub struct BodyValidator {
    maximum: Option<usize>,
    total: usize,
    line: usize,
    previous: Option<u8>,
    allow_eight_bit: bool,
    failed: bool,
}
impl BodyValidator {
    /// Create a text validator with an optional cumulative byte bound for the negotiated envelope mode.
    pub fn new(maximum: Option<usize>, allow_eight_bit: bool) -> Self {
        Self {
            maximum,
            total: 0,
            line: 0,
            previous: None,
            allow_eight_bit,
            failed: false,
        }
    }
    /// Validate one text chunk without retaining or transforming it.
    pub fn validate(&mut self, bytes: &[u8]) -> Result<(), crate::transport::smtp::Error> {
        use crate::transport::smtp::error;
        if self.failed {
            return Err(error::client("SMTP DATA encoder is retired"));
        }
        self.failed = true;
        self.total = self
            .total
            .checked_add(bytes.len())
            .filter(|n| self.maximum.is_none_or(|maximum| *n <= maximum))
            .ok_or_else(|| error::client("SMTP DATA exceeds its byte limit"))?;
        for &byte in bytes {
            if byte == 0
                || !self.allow_eight_bit && !byte.is_ascii()
                || byte == b'\n' && self.previous != Some(b'\r')
                || self.previous == Some(b'\r') && byte != b'\n'
            {
                return Err(error::client(
                    "SMTP DATA violates the envelope or CRLF framing",
                ));
            }
            self.line += 1;
            if self.line > 1000 {
                return Err(error::client("SMTP DATA line exceeds 1000 bytes"));
            }
            if byte == b'\n' {
                self.line = 0;
            }
            self.previous = Some(byte);
        }
        self.failed = false;
        Ok(())
    }
    /// Validate the end of a text message without normalizing it.
    pub fn finish(mut self) -> Result<(), crate::transport::smtp::Error> {
        self.failed |= self.total != 0 && self.previous != Some(b'\n');
        if self.failed {
            return Err(crate::transport::smtp::error::client(
                "Incomplete SMTP DATA",
            ));
        }
        Ok(())
    }
    /// Number of unencoded body bytes consumed.
    pub fn bytes(&self) -> usize {
        self.total
    }
}

/// Incremental DATA transparency using the existing SMTP codec.
pub struct DataEncoder {
    codec: ClientCodec,
    validator: BodyValidator,
}
impl DataEncoder {
    /// Create a DATA encoder with an optional cumulative byte bound for the negotiated envelope mode.
    pub fn new(maximum: Option<usize>, allow_eight_bit: bool) -> Self {
        Self {
            codec: ClientCodec::new(),
            validator: BodyValidator::new(maximum, allow_eight_bit),
        }
    }
    /// Validate and encode one chunk; no body is retained.
    pub fn encode(&mut self, bytes: &[u8]) -> Result<Vec<u8>, crate::transport::smtp::Error> {
        self.validator.validate(bytes)?;
        let mut output = Vec::with_capacity(bytes.len().checked_mul(2).ok_or_else(|| {
            crate::transport::smtp::error::client("SMTP DATA chunk is too large")
        })?);
        self.codec.encode(bytes, &mut output);
        Ok(output)
    }
    /// Finish the message and return its exact protocol terminator.
    pub fn finish(self) -> Result<&'static [u8], crate::transport::smtp::Error> {
        self.validator.finish()?;
        Ok(b".\r\n")
    }
    /// Count unencoded body bytes.
    pub fn bytes(&self) -> usize {
        self.validator.bytes()
    }
}

/// Returns the string replacing all the CRLF with "\<CRLF\>"
/// Used for debug displays
#[cfg(feature = "tracing")]
pub(super) fn escape_crlf(string: &str) -> String {
    string.replace("\r\n", "<CRLF>")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_codec() {
        let mut buf = Vec::new();
        let mut codec = ClientCodec::new();

        codec.encode(b".\r\n", &mut buf);
        codec.encode(b"test\r\n", &mut buf);
        codec.encode(b"test\r\n\r\n", &mut buf);
        codec.encode(b".\r\n", &mut buf);
        codec.encode(b"\r\ntest", &mut buf);
        codec.encode(b"te\r\n.\r\nst", &mut buf);
        codec.encode(b"test", &mut buf);
        codec.encode(b"test.", &mut buf);
        codec.encode(b"test\n", &mut buf);
        codec.encode(b".test\n", &mut buf);
        codec.encode(b"test", &mut buf);
        codec.encode(b"test", &mut buf);
        codec.encode(b"test\r\n", &mut buf);
        codec.encode(b".test\r\n", &mut buf);
        codec.encode(b"test.\r\n", &mut buf);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "..\r\ntest\r\ntest\r\n\r\n..\r\n\r\ntestte\r\n..\r\nsttesttest.test\n.test\ntesttesttest\r\n..test\r\ntest.\r\n"
        );
    }

    #[test]
    #[cfg(feature = "tracing")]
    fn test_escape_crlf() {
        assert_eq!(escape_crlf("\r\n"), "<CRLF>");
        assert_eq!(escape_crlf("EHLO my_name\r\n"), "EHLO my_name<CRLF>");
        assert_eq!(
            escape_crlf("EHLO my_name\r\nSIZE 42\r\n"),
            "EHLO my_name<CRLF>SIZE 42<CRLF>"
        );
    }
}

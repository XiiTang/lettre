# Boundless patches

Base: lettre v0.11.23, eb6136a06f1aba9432f73ef8f02a06aa0afc8f9c.

Boundless drives SMTP itself over a transport it supplies: it owns the socket,
TLS, SASL, deadlines and every command, and uses lettre's codec, reply parser
and connection for the wire. Ordinary upstream high-level clients keep their
API and behavior. Fork tests run with
`cargo test --no-default-features --features smtp-transport,tokio1 --lib --tests`
(the feature set Boundless uses; upstream doctests need a TLS backend). Boundless
tests live under its `protocols::smtp` module.

## Kept patches

### Supplied transport and its handoff

`AsyncSmtpConnection::from_transport` adopts a caller's Tokio stream without
greeting, EHLO, authentication, TLS, retry or QUIT, and `into_transport` returns
it after STARTTLS, refusing while buffered plaintext remains. The connection
reads through a one-byte buffer, so it never reads ahead across that handoff.
Fork test: `rejected_reply_is_data_and_handoff_preserves_next_bytes`. Boundless:
`independent_tls_mtls_and_starttls_verify_identity_trust_hostname_and_data`,
`required_starttls_is_not_downgraded_when_the_extension_is_missing`.

### Bounded reply reading

`read_response_fact` reads one reply and returns a negative reply as a
`Response`, not an error. Each line and the whole reply are bounded before they
are buffered, and the line must end in CRLF. Malformed input is not copied into
the error's text; the bytes read for the failing reply move into its
`UnparsedResponse`, zeroed on drop, for the caller to keep or drop. Upstream `read_response` reads through the
same path. `Response::into_parts` hands over code and lines without copying
reply text. A final reply may omit its text (`250\r\n`). Fork tests:
`malformed_and_oversized_replies_fail_without_echoing_input`,
`final_reply_may_omit_text`, `an_unread_reply_hands_its_bytes_to_the_caller`.

### Explicit writes and pipelining

`write_bytes` writes an already validated command, BDAT chunk or SASL response
without reading a reply. `pipeline` writes a group of commands while reading
their replies in order, gives each reply to the caller as it arrives, including
those before an I/O error, and stops writing when the caller says so (after a
421). Boundless: `pipeline_preserves_order_when_the_server_waits_for_the_complete_group`,
`failed_pipeline_keeps_received_replies_available_without_replay`,
`pipeline_drains_early_replies_while_large_bdat_is_backpressured`,
`independent_exim_pipelining_chunking_dsn_and_partial_recipients`.

### DATA through the upstream codec

`write_data` sends a body read from the caller's reader, up to a byte bound,
through a private `DataEncoder`: upstream's dot-stuffing codec plus the body
validator below, so CRLF split across reads is handled and the terminator adds
no blank line. Boundless passes a body it already holds.
Invalid input retires the connection; nothing is normalized, reset or resent.
Fork unit tests: `data_encoder_handles_crlf_split_and_terminal_errors`,
`data_encoder_optional_bound_keeps_validation`. Boundless:
`partial_recipient_rejection_preserves_replies_and_data_is_dot_stuffed`,
`smtp_input_rejects_command_smuggling_and_data_normalization`.

### Shared body validation

`BodyValidator` checks CRLF framing, NUL, 8-bit use against the envelope, 1000
byte lines and an optional cumulative byte bound, incrementally and without
retaining the body. It is `Copy` so a caller can check a body's end without
giving up its state. Boundless validates text BDAT with it, with no dot
transparency and no assumption that chunk boundaries are line boundaries.
Fork test: `chunked_text_validation_has_no_dot_transparency_or_chunk_boundary_assumptions`.
Boundless: `text_bdat_uses_shared_validation_without_dot_stuffing`.

### No written bytes in tracing

With the `tracing` feature, upstream logs every written line, AUTH responses and
message bodies included. The connection logs the length of a write instead.
Boundless builds without `tracing`, but `write_bytes` exists for SASL
responses and `write_data` for bodies, so the redaction is what makes those
APIs safe for any build that enables it. Evidence:
`cargo check --no-default-features --features smtp-transport,tokio1,tracing`.

### Tests in Boundless's feature set

Upstream's transport tests need a TLS backend; they are gated on one, so the
library tests build and run without TLS.

## History

2026-09-14: `BodyValidator::new` takes `Option<usize>`; `None` imposes no
cumulative byte bound, other checks unchanged.
2026-10-10: replies are returned without the raw bytes they were read from,
and `DataEncoder` became private to `write_data`.

## Fixed locked certificate verification (#312)

Update rustls-webpki 0.103.13 to 0.103.16 and webpki-root-certs 1.0.9 to
1.0.10 for RUSTSEC-2026-0382 and RUSTSEC-2026-0383. webpki now requires
aws-lc-rs 1.18; retain the minimum 1.18.0, aws-lc-sys 0.44.0 and optional
aws-lc-fips-sys 0.14.0. No SMTP, transport or public API code changes.
Regression coverage: the builder/SMTP/Tokio/ring/platform-verifier suite and
Boundless's complete SMTP library tests.

## W5-D10: unreadable reply facts

Boundless needs the exact bytes of an unreadable public reply without copying
mechanism input into error text. The bounded reader moves its existing zeroized
buffer into UnparsedResponse, with completeness fixed at the deciding exit.
Display exposes only the static reason; Debug exposes reason and length.
Regression evidence: an_unread_reply_hands_its_bytes_to_the_caller covers every
reader exit, truncation and exact bytes. The supplied-transport handoff and
malformed-input regression tests remain in the required feature-set suite.

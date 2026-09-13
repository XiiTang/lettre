# Runtime supplied transport patch

Base: lettre v0.11.23, eb6136a06f1aba9432f73ef8f02a06aa0afc8f9c.

- Adopt and recover an existing Tokio stream without implicit greeting, authentication, TLS, retry or QUIT. The caller owns destination authorization and transport security.
- Separate bounded reply reading from writing for explicit pipelines and BDAT. Negative SMTP replies remain response facts; the upstream parser remains authoritative.
- Stream DATA through the upstream dot-stuffing codec with cross-chunk CRLF, line, byte and transfer-mode validation. The exact terminator does not add a blank message line.
- Avoid read-ahead across STARTTLS handoff. Reply limits apply before allocating an unbounded line; malformed input is not copied into errors.
- Authentication/body writes are never included in tracing. Transient reply buffers are zeroed. Ordinary upstream high-level clients retain their existing success/error API.
- Accept a final reply code without optional text. Gate upstream TLS-only tests on their required features.

Validation: 32 library tests and 5 supplied-transport integration tests pass with `--no-default-features --features smtp-transport,tokio1`; IMAPipe SMTP tests cover partial recipient rejection, explicit STARTTLS, frozen authentication failure, DATA streaming, BDAT and ordered pipeline results.

Follow-up: DATA and text BDAT share one incremental body validator; BDAT validation never adds dot transparency or assumes chunk boundaries are line boundaries.

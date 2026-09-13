#![cfg(all(feature = "smtp-transport", feature = "tokio1"))]
use lettre::transport::smtp::client::{AsyncSmtpConnection, DataEncoder};
use tokio1_crate::{
    self as tokio,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn rejected_reply_is_data_and_handoff_preserves_next_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    peer.write_all(b"550-no user\r\n550 rejected\r\nNEXT")
        .await
        .unwrap();
    let mut client = AsyncSmtpConnection::from_transport(Box::new(socket));
    let (reply, bytes) = client.read_response_fact().await.unwrap();
    assert_eq!(u16::from(reply.code()), 550);
    assert_eq!(bytes, b"550-no user\r\n550 rejected\r\n");
    let mut socket = client.into_transport().unwrap();
    let mut next = [0; 4];
    socket.read_exact(&mut next).await.unwrap();
    assert_eq!(&next, b"NEXT");
    drop(socket);
    let mut extra = Vec::new();
    peer.read_to_end(&mut extra).await.unwrap();
    assert!(extra.is_empty(), "drop must not send QUIT or RSET");
}
#[tokio::test]
async fn malformed_and_oversized_replies_fail_without_echoing_input() {
    for bytes in [
        b"550-first\r\n250 changed\r\n".to_vec(),
        b"250"
            .iter()
            .copied()
            .chain(vec![b'X'; 2000])
            .chain(b"\r\n".iter().copied())
            .collect(),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut peer, _) = listener.accept().await.unwrap();
        peer.write_all(&bytes).await.unwrap();
        let mut client = AsyncSmtpConnection::from_transport(Box::new(socket));
        let error = client.read_response_fact().await.unwrap_err().to_string();
        assert!(!error.contains("first"));
        assert!(!error.contains("XXXX"));
    }
}
#[test]
fn streaming_encoder_handles_crlf_split_and_terminal_errors() {
    let mut encoder = DataEncoder::new(100, false);
    let mut out = encoder.encode(b".a\r").unwrap();
    out.extend(encoder.encode(b"\n.").unwrap());
    out.extend(encoder.encode(b"b\r\n").unwrap());
    out.extend(encoder.finish().unwrap());
    assert_eq!(out, b"..a\r\n..b\r\n.\r\n");
    let mut encoder = DataEncoder::new(3, false);
    assert!(encoder.encode(b"1234").is_err());
    assert!(encoder.encode(b"\r\n").is_err());
    assert!(encoder.finish().is_err());
    let mut encoder = DataEncoder::new(100, false);
    assert!(encoder.encode(b"bad\n").is_err());
    let mut encoder = DataEncoder::new(100, false);
    assert!(encoder.encode(&[0xff]).is_err());
}
#[tokio::test]
async fn final_reply_may_omit_text() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    peer.write_all(b"250\r\n").await.unwrap();
    let mut client = AsyncSmtpConnection::from_transport(Box::new(socket));
    assert_eq!(
        u16::from(client.read_response_fact().await.unwrap().0.code()),
        250
    );
}

#[test]
fn chunked_text_validation_has_no_dot_transparency_or_chunk_boundary_assumptions() {
    use lettre::transport::smtp::client::BodyValidator;
    let mut body = BodyValidator::new(100, false);
    body.validate(b"a\r").unwrap();
    body.validate(b"\n.dot\r\n").unwrap();
    body.finish().unwrap();
    let mut body = BodyValidator::new(100, true);
    body.validate(b"unfinished").unwrap();
    assert!(body.finish().is_err());
}

//! Endpoint descriptor parse tests (added per review suggestion: previously only covered
//! indirectly by the default branch).

use slsdk_rs::endpoint::{Endpoint, EndpointParseError};

#[test]
fn tcp_descriptor_parses() {
    let ep: Endpoint = "tcp:127.0.0.1:39876".parse().unwrap();
    assert!(matches!(ep, Endpoint::Tcp { ref addr } if addr.port() == 39876));
    // Display is the inverse of the descriptor format
    assert_eq!(ep.to_string(), "tcp:127.0.0.1:39876");
}

#[test]
fn uds_descriptor_parses() {
    let ep: Endpoint = "uds:/tmp/slsdk-test.sock".parse().unwrap();
    assert!(matches!(ep, Endpoint::Uds { ref path } if path == "/tmp/slsdk-test.sock"));
    assert_eq!(ep.to_string(), "uds:/tmp/slsdk-test.sock");
}

#[test]
fn tcp_invalid_addr_errors() {
    let err = "tcp:not-an-addr".parse::<Endpoint>().unwrap_err();
    assert!(matches!(err, EndpointParseError::InvalidTcpAddr(_)));
}

#[test]
fn missing_scheme_errors() {
    let err = "127.0.0.1:39876".parse::<Endpoint>().unwrap_err();
    assert!(matches!(err, EndpointParseError::MissingScheme(_)));
}

#[test]
fn unknown_scheme_falls_to_missing_scheme() {
    // from_str only recognizes the uds:/tcp: prefixes; everything else (including grpc:)
    // falls into MissingScheme
    let err = "grpc:127.0.0.1:39876".parse::<Endpoint>().unwrap_err();
    assert!(matches!(err, EndpointParseError::MissingScheme(_)));
}

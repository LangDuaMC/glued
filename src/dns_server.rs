//! DNS server subsystem.
//!
//! This module implements a lightweight DNS server using the
//! [hickory-dns](https://crates.io/crates/hickory-dns-server) library.
//! The server listens on a configurable UDP/TCP socket and
//! processes DNS queries as follows:
//!
//! * **Single‑label names** (no dots): treated as container names.  The
//!   server looks up the name in the shared state map and, if
//!   present, returns an A or AAAA record with the container's IP.
//! * **FQDNs** (names containing a dot): forwarded to upstream
//!   resolvers using the `hickory-resolver` crate.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use hickory_resolver::TokioAsyncResolver;
use hickory_server::authority::MessageResponseBuilder;
use hickory_server::proto::op::{Header, ResponseCode};
use hickory_server::proto::rr::rdata::{A, AAAA};
use hickory_server::proto::rr::{RData, Record, RecordType};
use hickory_server::server::{
    Request, RequestHandler, ResponseHandler, ResponseInfo, ServerFuture,
};
use log::{error, info, warn};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::RwLock;
use tokio::time::Duration;

/// Timeout for idle TCP connections.
const TCP_TIMEOUT: Duration = Duration::from_secs(10);

/// Start the DNS server.
pub async fn run_dns_server(
    bind_addr: SocketAddr,
    state: Arc<RwLock<HashMap<String, String>>>,
) -> anyhow::Result<()> {
    info!("DNS server starting on {}", bind_addr);

    // Create a system resolver for forwarding FQDNs.
    let resolver = TokioAsyncResolver::tokio_from_system_conf().unwrap_or_else(|e| {
        error!(
            "Failed to load system resolv.conf: {}. Falling back to Google DNS.",
            e
        );
        TokioAsyncResolver::tokio_from_system_conf().unwrap_or_else(|_| {
            panic!("Failed to create DNS resolver: {}", e);
        })
    });

    let handler = GluedDns { state, resolver };
    let mut server = ServerFuture::new(handler);

    // Register UDP listener.
    let udp = UdpSocket::bind(bind_addr).await?;
    server.register_socket(udp);

    // Register TCP listener.
    let tcp = TcpListener::bind(bind_addr).await?;
    server.register_listener(tcp, TCP_TIMEOUT);

    // Run the server until future resolves.
    server.block_until_done().await?;
    Ok(())
}

struct GluedDns {
    state: Arc<RwLock<HashMap<String, String>>>,
    resolver: TokioAsyncResolver,
}

#[async_trait]
impl RequestHandler for GluedDns {
    async fn handle_request<R>(&self, request: &Request, mut response_handle: R) -> ResponseInfo
    where
        R: ResponseHandler + Send,
    {
        let query = request.query();
        let qname = query.name().to_string().trim_end_matches('.').to_string();
        let qtype = query.query_type();

        // Build response header
        let mut header = Header::response_from_request(request.header());
        header.set_recursion_available(true);

        // Single-label check
        let is_single_label = !qname.contains('.');
        if is_single_label {
            let ip_opt = {
                let map = self.state.read().await;
                map.get(&qname).cloned()
            };

            match ip_opt {
                Some(ip_str) => match ip_str.parse::<std::net::IpAddr>() {
                    Ok(std::net::IpAddr::V4(ipv4)) => {
                        if qtype == RecordType::A || qtype == RecordType::ANY {
                            let record = Record::from_rdata(
                                query.name().clone().into(),
                                5,
                                RData::A(A(ipv4)),
                            );
                            let builder = MessageResponseBuilder::from_message_request(request);
                            let records = [record];
                            let response = builder.build(
                                header,
                                records.iter(),
                                std::iter::empty(),
                                std::iter::empty(),
                                std::iter::empty(),
                            );
                            return response_handle.send_response(response).await.unwrap();
                        } else {
                            header.set_response_code(ResponseCode::NoError);
                            let builder = MessageResponseBuilder::from_message_request(request);
                            let response = builder.build_no_records(header);
                            return response_handle.send_response(response).await.unwrap();
                        }
                    }
                    Ok(std::net::IpAddr::V6(ipv6)) => {
                        if qtype == RecordType::AAAA || qtype == RecordType::ANY {
                            let record = Record::from_rdata(
                                query.name().clone().into(),
                                5,
                                RData::AAAA(AAAA(ipv6)),
                            );
                            let builder = MessageResponseBuilder::from_message_request(request);
                            let records = [record];
                            let response = builder.build(
                                header,
                                records.iter(),
                                std::iter::empty(),
                                std::iter::empty(),
                                std::iter::empty(),
                            );
                            return response_handle.send_response(response).await.unwrap();
                        } else {
                            header.set_response_code(ResponseCode::NoError);
                            let builder = MessageResponseBuilder::from_message_request(request);
                            let response = builder.build_no_records(header);
                            return response_handle.send_response(response).await.unwrap();
                        }
                    }
                    Err(_) => {
                        header.set_response_code(ResponseCode::ServFail);
                        let builder = MessageResponseBuilder::from_message_request(request);
                        let response = builder.build_no_records(header);
                        return response_handle.send_response(response).await.unwrap();
                    }
                },
                None => {
                    header.set_response_code(ResponseCode::NXDomain);
                    let builder = MessageResponseBuilder::from_message_request(request);
                    let response = builder.build_no_records(header);
                    return response_handle.send_response(response).await.unwrap();
                }
            }
        }

        // Forward FQDN
        match self.resolver.lookup_ip(qname.clone()).await {
            Ok(lookup) => {
                let mut records = Vec::new();
                for addr in lookup.iter() {
                    let addr: std::net::IpAddr = addr;
                    match (addr, qtype) {
                        (std::net::IpAddr::V4(ipv4), RecordType::A)
                        | (std::net::IpAddr::V4(ipv4), RecordType::ANY) => {
                            records.push(Record::from_rdata(
                                query.name().clone().into(),
                                60,
                                RData::A(A(ipv4)),
                            ));
                        }
                        (std::net::IpAddr::V6(ipv6), RecordType::AAAA)
                        | (std::net::IpAddr::V6(ipv6), RecordType::ANY) => {
                            records.push(Record::from_rdata(
                                query.name().clone().into(),
                                60,
                                RData::AAAA(AAAA(ipv6)),
                            ));
                        }
                        _ => {}
                    }
                }
                header.set_response_code(ResponseCode::NoError);
                let builder = MessageResponseBuilder::from_message_request(request);
                let response = builder.build(
                    header,
                    records.iter(),
                    std::iter::empty(),
                    std::iter::empty(),
                    std::iter::empty(),
                );
                response_handle.send_response(response).await.unwrap()
            }
            Err(e) => {
                warn!("Resolver lookup failed for {}: {}", qname, e);
                header.set_response_code(ResponseCode::ServFail);
                let builder = MessageResponseBuilder::from_message_request(request);
                let response = builder.build_no_records(header);
                response_handle.send_response(response).await.unwrap()
            }
        }
    }
}

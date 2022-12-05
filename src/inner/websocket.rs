use futures_util::{SinkExt, StreamExt};
use http::header::HeaderName;
use http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use hyper::upgrade::{OnUpgrade, Upgraded};
use sha1::{Digest, Sha1};
use tokio_tungstenite::tungstenite::protocol::{self, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use crate::inner::InnerStranglerService;
use crate::WebSocketScheme;

#[cfg(feature = "websocket")]
impl<C> InnerStranglerService<C> {
    /// Implementation adapted from `axum::extract::ws`
    pub(super) async fn handle_websocket_upgrade_request(
        &self,
        req: http::Request<hyper::body::Body>,
    ) -> Result<axum_core::response::Response, http::Request<hyper::body::Body>> {
        let (mut parts, body) = req.into_parts();
        if parts.method != http::Method::GET {
            return Err(http::Request::from_parts(parts, body));
        }

        if !header_contains(&parts.headers, header::CONNECTION, "upgrade")
            || !header_eq(&parts.headers, header::UPGRADE, "websocket")
            || !header_eq(&parts.headers, header::SEC_WEBSOCKET_VERSION, "13")
        {
            return Err(http::Request::from_parts(parts, body));
        }

        let sec_websocket_key = parts
            .headers
            .remove(header::SEC_WEBSOCKET_KEY)
            .ok_or_else(|| todo!())
            .unwrap();

        let on_upgrade = parts
            .extensions
            .remove::<OnUpgrade>()
            .ok_or_else(|| todo!())
            .unwrap();

        let sec_websocket_protocol = parts.headers.get(header::SEC_WEBSOCKET_PROTOCOL).cloned();

        let strangled_authority = self.strangled_authority.clone();
        let strangled_scheme = match self.strangled_web_socket_scheme {
            WebSocketScheme::WS => "ws",
            #[cfg(any(
                feature = "websocket-native-tls",
                feature = "websocket-rustls-tls-native-roots",
                feature = "websocket-rustls-tls-webpki-roots"
            ))]
            WebSocketScheme::WSS => "wss",
        };

        let uri = http::Uri::builder()
            .authority(strangled_authority)
            .scheme(strangled_scheme)
            .path_and_query(parts.uri.path_and_query().cloned().unwrap())
            .build()
            .unwrap();

        let websocketconfig = WebSocketConfig::default();

        let protocol = sec_websocket_protocol.clone();

        let (connection, _) = tokio_tungstenite::connect_async(uri).await.unwrap();

        tokio::spawn(async move {
            let upgraded = on_upgrade.await.expect("connection upgrade failed");
            let socket = WebSocketStream::from_raw_socket(
                upgraded,
                protocol::Role::Server,
                Some(websocketconfig),
            )
            .await;
            on_websocket_upgrade(socket, connection).await;
        });

        #[allow(clippy::declare_interior_mutable_const)]
        const UPGRADE: HeaderValue = HeaderValue::from_static("upgrade");
        #[allow(clippy::declare_interior_mutable_const)]
        const WEBSOCKET: HeaderValue = HeaderValue::from_static("websocket");

        let mut builder = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, UPGRADE)
            .header(header::UPGRADE, WEBSOCKET)
            .header(
                header::SEC_WEBSOCKET_ACCEPT,
                sign(sec_websocket_key.as_bytes()),
            );

        if let Some(protocol) = protocol {
            builder = builder.header(header::SEC_WEBSOCKET_PROTOCOL, protocol);
        }

        Ok(builder
            .body(axum_core::body::boxed(hyper::Body::empty()))
            .unwrap())
    }
}

/// Implementation taken from `axum::extract::ws`
fn sign(key: &[u8]) -> HeaderValue {
    let mut sha1 = Sha1::default();
    sha1.update(key);
    sha1.update(&b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"[..]);
    let b64 = base64::encode(sha1.finalize());
    HeaderValue::from_maybe_shared(b64).expect("base64 is a valid value")
}

fn header_eq(headers: &HeaderMap, key: HeaderName, value: &'static str) -> bool {
    if let Some(header) = headers.get(&key) {
        header.as_bytes().eq_ignore_ascii_case(value.as_bytes())
    } else {
        false
    }
}

fn header_contains(headers: &HeaderMap, key: HeaderName, value: &'static str) -> bool {
    let header = if let Some(header) = headers.get(&key) {
        header
    } else {
        return false;
    };

    if let Ok(header) = std::str::from_utf8(header.as_bytes()) {
        header.to_ascii_lowercase().contains(value)
    } else {
        false
    }
}

async fn on_websocket_upgrade(
    socket: WebSocketStream<Upgraded>,
    strangled_websocket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut strangled_sink, mut strangled_stream) = strangled_websocket.split();
    let (mut client_sink, mut client_stream) = socket.split();

    let (sender1, mut receiver1) = tokio::sync::broadcast::channel::<()>(1);
    let (sender2, mut receiver2) = tokio::sync::broadcast::channel::<()>(1);

    let from_strangled_to_client_jh = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(Ok(msg)) = strangled_stream.next() => {
                    match client_sink
                        .send(msg)
                        .await {
                            Ok(_) => {},
                            Err(e) => {
                                dbg!(e);
                                sender2.send(()).unwrap();
                                return;
                            }
                        }
                }
                _ = receiver1.recv() => {
                    return;
                }
                else => {
                    sender2.send(()).unwrap();
                    return;
                }
            }
        }
    });

    let from_client_to_strangled_jh = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(Ok(msg)) = client_stream.next() => {
                    match strangled_sink
                        .send(msg)
                        .await
                        {
                            Ok(_) => {},
                            Err(e) => {
                                dbg!(e);
                                sender1.send(()).unwrap();
                                return;
                            }
                        }
                }
                _ = receiver2.recv() => {
                    return;
                }
                else => {
                    sender1.send(()).unwrap();
                    return;
                }
            }
        }
    });

    from_client_to_strangled_jh.await.unwrap();
    from_strangled_to_client_jh.await.unwrap();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::*;
    use axum::{routing::get, Router};

    use futures_util::{SinkExt, StreamExt};

    #[tokio::test]
    async fn proxies_strangled_websocket_service() {
        // helper function
        async fn handle_socket(mut socket: axum::extract::ws::WebSocket) {
            // Only reply to a single message and stop.
            if let Some(Ok(msg)) = socket.recv().await {
                if socket.send(msg).await.is_err() {
                    // client disconnected
                    return;
                }
            }
        }

        let router = Router::new().route(
            "/api/websocket",
            get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|socket| handle_socket(socket))
            }),
        );

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let _stranglee = tokio::spawn(async move {
            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let strangler_svc = Strangler::new(
            axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port)).unwrap(),
        );

        let _strangler = tokio::spawn(async move {
            let router = Router::new().fallback_service(strangler_svc);
            axum::Server::from_tcp(strangler_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let message = "A websocket message";
        let (mut ws_connection, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/api/websocket",
            strangler_port
        ))
        .await
        .unwrap();

        ws_connection
            .send(tokio_tungstenite::tungstenite::protocol::Message::Text(
                message.to_owned(),
            ))
            .await
            .unwrap();
        let msg = ws_connection.next().await.unwrap().unwrap();
        match msg {
            tokio_tungstenite::tungstenite::Message::Text(response) => {
                assert_eq!(message, &response);
            }
            _ => panic!("We expected text back but got something else!"),
        }
    }

    #[tokio::test]
    async fn proxied_websocket_closes_properly_close_from_strangled() {
        // helper function
        async fn handle_socket(socket: axum::extract::ws::WebSocket) {
            // Immediately close
            socket.close().await.unwrap();
        }

        let router = Router::new().route(
            "/api/websocket",
            get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|socket| handle_socket(socket))
            }),
        );

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let _stranglee = tokio::spawn(async move {
            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let strangler_svc = Strangler::new(
            axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port)).unwrap(),
        );

        let _strangler = tokio::spawn(async move {
            let router = Router::new().fallback_service(strangler_svc);
            axum::Server::from_tcp(strangler_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let (mut ws_connection, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/api/websocket",
            strangler_port
        ))
        .await
        .unwrap();

        let msg = ws_connection.next().await.unwrap().unwrap();
        assert!(msg.is_close());
    }

    #[tokio::test]
    async fn proxied_websocket_closes_properly_close_from_client() {
        // helper function
        async fn handle_socket(_socket: axum::extract::ws::WebSocket) {
            // We don't get here, so doesn't matter:
            panic!("Shouldn't get here when socket gets closed by other side?")
        }

        let router = Router::new().route(
            "/api/websocket",
            get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|socket| handle_socket(socket))
            }),
        );

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let _stranglee = tokio::spawn(async move {
            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let strangler_svc = Strangler::new(
            axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port)).unwrap(),
        );

        let _strangler = tokio::spawn(async move {
            let router = Router::new().fallback_service(strangler_svc);
            axum::Server::from_tcp(strangler_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        let (mut ws_connection, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/api/websocket",
            strangler_port
        ))
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        ws_connection.close(None).await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

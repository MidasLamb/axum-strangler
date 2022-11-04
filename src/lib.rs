//! # Axum Strangler
//! A little utility to do the Strangler Fig pattern in the Axum framework without having to use
//! some gateway.
//! This makes "strangling" a bit easier, as everything that is handled by the "strangler" will
//! automatically no longer be forwarded to the "stranglee" (a.k.a. the old service).

use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    extract::{ws::WebSocket, RequestParts},
    http::Uri,
};
use futures_util::{SinkExt, StreamExt};
use tower_service::Service;

#[derive(Clone)]
pub enum SchemeSecurity {
    None,
    Https,
    Wss,
    HttpsAndWss,
}

/// Service that forwards all requests to another service
/// ```rust
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let strangler_svc = axum_strangler::StranglerService::new(
///         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
///         axum_strangler::SchemeSecurity::None,
///     );
///     let router = axum::Router::new().fallback(strangler_svc);
///     axum::Server::bind(&"127.0.0.1:0".parse()?)
///         .serve(router.into_make_service())
///         # .with_graceful_shutdown(async {
///         # // Shut down immediately
///         # })
///         .await?;
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct StranglerService {
    http_client: hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>,
    inner: Arc<InnerStranglerService>,
}

impl StranglerService {
    /// Construct a new `StranglerService`.
    /// * The `strangled_authority` is the host & port of the service to be strangled.
    /// * The `strangled_scheme` is which scheme the strangled service has to make
    pub fn new(
        strangled_authority: axum::http::uri::Authority,
        strangled_scheme_security: SchemeSecurity,
    ) -> Self {
        let https = hyper_tls::HttpsConnector::new();
        let client = hyper::Client::builder().build::<_, hyper::Body>(https);
        Self {
            http_client: client,
            inner: Arc::new(InnerStranglerService {
                strangled_authority,
                strangled_scheme_security,
            }),
        }
    }
}

struct InnerStranglerService {
    strangled_authority: axum::http::uri::Authority,
    strangled_scheme_security: SchemeSecurity,
}

impl Service<axum::http::Request<axum::body::Body>> for StranglerService {
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let http_client = self.http_client.clone();
        let inner = self.inner.clone();

        let fut = forward_call_to_strangled(http_client, inner, req);
        Box::pin(fut)
    }
}

#[tracing::instrument(skip_all, fields(req.path = %req.uri()))] // Note that we set the path to the
                                                                // "full" uri, as host etc gets
                                                                // removed by axum already.
async fn forward_call_to_strangled(
    http_client: hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>,
    inner: Arc<InnerStranglerService>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, Infallible> {
    tracing::info!("handling a request");
    let mut request_parts = RequestParts::new(req);
    let wsu: Result<axum::extract::ws::WebSocketUpgrade, _> = request_parts.extract().await;

    let req: Result<axum::http::Request<axum::body::Body>, _> = request_parts.extract().await;
    let mut req = req.unwrap();

    let uri: Uri = match wsu {
        Ok(wsu) => {
            tracing::info!("Handling websocket request");
            let strangled_authority = inner.strangled_authority.clone();
            let strangled_scheme = match inner.strangled_scheme_security {
                SchemeSecurity::None | SchemeSecurity::Https => "ws",
                SchemeSecurity::Wss | SchemeSecurity::HttpsAndWss => "wss",
            };

            let uri = Uri::builder()
                .authority(strangled_authority)
                .scheme(strangled_scheme)
                .path_and_query(req.uri().path_and_query().cloned().unwrap())
                .build()
                .unwrap();

            let (connection, _) = tokio_tungstenite::connect_async(uri).await.unwrap();
            return Ok(wsu.on_upgrade(|socket| on_websocket_upgrade(socket, connection)));
        }
        Err(_) => {
            // Not really anything to do, because this could just not be a websocket
            // request.
            let strangled_authority = inner.strangled_authority.clone();
            let strangled_scheme = match inner.strangled_scheme_security {
                SchemeSecurity::None | SchemeSecurity::Wss => axum::http::uri::Scheme::HTTP,
                SchemeSecurity::Https | SchemeSecurity::HttpsAndWss => {
                    axum::http::uri::Scheme::HTTPS
                }
            };
            Uri::builder()
                .scheme(strangled_scheme)
                .authority(strangled_authority)
                .path_and_query(req.uri().path_and_query().cloned().unwrap())
                .build()
                .unwrap()
        }
    };

    // Change the host header so any sort of other proxy in between can route correctly based on
    // hostname.
    if let Some(host) = req.headers_mut().get_mut("host") {
        *host = axum::http::HeaderValue::from_str(uri.authority().unwrap().as_str()).unwrap()
    }

    *req.uri_mut() = uri;

    let r = http_client.request(req).await.unwrap();

    let mut response_builder = axum::response::Response::builder();
    response_builder = response_builder.status(r.status());

    if let Some(headers) = response_builder.headers_mut() {
        *headers = r.headers().clone();
    }

    let response = response_builder
        .body(axum::body::boxed(r))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR);

    match response {
        Ok(response) => Ok(response),
        Err(_) => todo!(),
    }
}

fn translate_to_axum(
    message: tokio_tungstenite::tungstenite::protocol::Message,
) -> axum::extract::ws::Message {
    match message {
        tokio_tungstenite::tungstenite::Message::Text(t) => axum::extract::ws::Message::Text(t),
        tokio_tungstenite::tungstenite::Message::Binary(b) => axum::extract::ws::Message::Binary(b),
        tokio_tungstenite::tungstenite::Message::Ping(p) => axum::extract::ws::Message::Ping(p),
        tokio_tungstenite::tungstenite::Message::Pong(p) => axum::extract::ws::Message::Pong(p),
        tokio_tungstenite::tungstenite::Message::Close(c) => {
            let close_message = c.map(|c| axum::extract::ws::CloseFrame {
                reason: c.reason,
                code: tungstenite_close_code_to_axum(c.code),
            });
            axum::extract::ws::Message::Close(close_message)
        }
        tokio_tungstenite::tungstenite::Message::Frame(_f) => todo!(),
    }
}
fn translate_to_tungstenite(
    message: axum::extract::ws::Message,
) -> tokio_tungstenite::tungstenite::protocol::Message {
    match message {
        axum::extract::ws::Message::Text(t) => {
            tokio_tungstenite::tungstenite::protocol::Message::Text(t)
        }
        axum::extract::ws::Message::Binary(b) => {
            tokio_tungstenite::tungstenite::protocol::Message::Binary(b)
        }
        axum::extract::ws::Message::Ping(p) => {
            tokio_tungstenite::tungstenite::protocol::Message::Ping(p)
        }
        axum::extract::ws::Message::Pong(p) => {
            tokio_tungstenite::tungstenite::protocol::Message::Pong(p)
        }
        axum::extract::ws::Message::Close(c) => {
            let close_message =
                c.map(
                    |c| tokio_tungstenite::tungstenite::protocol::frame::CloseFrame {
                        reason: c.reason,
                        code: axum_close_code_to_tungstenite(c.code),
                    },
                );
            tokio_tungstenite::tungstenite::protocol::Message::Close(close_message)
        }
    }
}

fn axum_close_code_to_tungstenite(
    code: axum::extract::ws::CloseCode,
) -> tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode {
    tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code)
}

fn tungstenite_close_code_to_axum(
    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode,
) -> axum::extract::ws::CloseCode {
    code.into()
}

#[tracing::instrument(skip_all)]
async fn on_websocket_upgrade(
    socket: WebSocket,
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
                        .send(translate_to_axum(msg))
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
                        .send(translate_to_tungstenite(msg))
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
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Arc;

    use super::*;
    use axum::{routing::get, Extension, Router};

    /// Create a mock service that's not connecting to anything.
    fn make_svc() -> StranglerService {
        StranglerService::new(
            axum::http::uri::Authority::from_static("127.0.0.1:0"),
            SchemeSecurity::None,
        )
    }

    #[tokio::test]
    async fn can_be_used_as_fallback() {
        let router = Router::new().fallback(make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[tokio::test]
    async fn can_be_used_for_a_route() {
        let router = Router::new().route("/api", make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[derive(Clone)]
    struct StopChannel(Arc<tokio::sync::broadcast::Sender<()>>);

    struct StartupHelper {
        strangler_port: u16,
        strangler_joinhandle: tokio::task::JoinHandle<()>,
        stranglee_joinhandle: tokio::task::JoinHandle<()>,
    }

    async fn start_up_strangler_and_strangled(strangled_router: Router) -> StartupHelper {
        let (tx, mut rx_1) = tokio::sync::broadcast::channel::<()>(1);
        let mut rx_2 = tx.subscribe();
        let tx_arc = Arc::new(tx);
        let stop_channel = StopChannel(tx_arc);

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let https = hyper_tls::HttpsConnector::new();
        let client = hyper::Client::builder().build::<_, hyper::Body>(https);
        let strangler_svc = StranglerService {
            http_client: client,
            inner: Arc::new(InnerStranglerService {
                strangled_authority: axum::http::uri::Authority::try_from(format!(
                    "127.0.0.1:{}",
                    stranglee_port
                ))
                .unwrap(),
                strangled_scheme_security: SchemeSecurity::None,
            }),
        };

        let background_stranglee_handle = tokio::spawn(async move {
            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(
                    strangled_router
                        .layer(axum::Extension(stop_channel))
                        .into_make_service(),
                )
                .with_graceful_shutdown(async {
                    rx_1.recv().await.ok();
                })
                .await
                .unwrap();
        });

        let background_strangler_handle = tokio::spawn(async move {
            let router = Router::new().fallback(strangler_svc);
            axum::Server::from_tcp(strangler_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .with_graceful_shutdown(async {
                    rx_2.recv().await.ok();
                })
                .await
                .unwrap();
        });

        StartupHelper {
            strangler_port,
            strangler_joinhandle: background_strangler_handle,
            stranglee_joinhandle: background_stranglee_handle,
        }
    }

    #[tokio::test]
    async fn proxies_strangled_http_service() {
        let router = Router::new().route(
            "/api/something",
            get(
                |Extension(StopChannel(tx_arc)): Extension<StopChannel>| async move {
                    tx_arc.send(()).unwrap();
                    "I'm being strangled"
                },
            ),
        );

        let StartupHelper {
            strangler_port,
            strangler_joinhandle,
            stranglee_joinhandle,
        } = start_up_strangler_and_strangled(router).await;

        let c = reqwest::Client::new();
        let r = c
            .get(format!("http://127.0.0.1:{}/api/something", strangler_port))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        assert_eq!(r, "I'm being strangled");

        stranglee_joinhandle.await.unwrap();
        strangler_joinhandle.await.unwrap();
    }

    #[tokio::test]
    async fn proxies_strangled_websocket_service() {
        // helper function
        async fn handle_socket(
            mut socket: WebSocket,
            tx_arc: Arc<tokio::sync::broadcast::Sender<()>>,
        ) {
            // Only reply to a single message and stop.
            if let Some(Ok(msg)) = socket.recv().await {
                if socket.send(msg).await.is_err() {
                    // client disconnected
                    return;
                }
            }
            tx_arc.send(()).unwrap();
        }

        let router = Router::new().route(
            "/api/websocket",
            get(
                |ws: axum::extract::ws::WebSocketUpgrade,
                 Extension(StopChannel(tx_arc)): Extension<StopChannel>| async move {
                    ws.on_upgrade(|socket| handle_socket(socket, tx_arc))
                },
            ),
        );

        let StartupHelper {
            strangler_port,
            strangler_joinhandle,
            stranglee_joinhandle,
        } = start_up_strangler_and_strangled(router).await;

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

        stranglee_joinhandle.await.unwrap();
        strangler_joinhandle.await.unwrap();
    }
}

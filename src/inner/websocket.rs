use axum::extract::ws::Message as AxumMessage;
use axum::{
    extract::{ws::WebSocket, RequestParts},
    http::Uri,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;

use crate::inner::InnerStranglerService;
use crate::SchemeSecurity;

#[cfg(feature = "websocket")]
impl InnerStranglerService {
    pub(super) fn get_http_scheme(&self) -> axum::http::uri::Scheme {
        match self.strangled_scheme_security {
            SchemeSecurity::None | SchemeSecurity::Wss => axum::http::uri::Scheme::HTTP,
            SchemeSecurity::Https | SchemeSecurity::HttpsAndWss => axum::http::uri::Scheme::HTTPS,
        }
    }

    pub(super) async fn handle_websocket_upgrade_request(
        &self,
        req: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, axum::http::Request<axum::body::Body>> {
        let mut request_parts = RequestParts::new(req);
        let wsu: axum::extract::ws::WebSocketUpgrade = match request_parts.extract().await {
            Ok(wsu) => wsu,
            Err(_) => {
                let req: Result<axum::http::Request<axum::body::Body>, _> =
                    request_parts.extract().await;
                return Err(req.unwrap());
            }
        };

        let req: Result<axum::http::Request<axum::body::Body>, _> = request_parts.extract().await;
        let req = req.unwrap();

        let strangled_authority = self.strangled_authority.clone();
        let strangled_scheme = match self.strangled_scheme_security {
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
}

trait Axumable {
    fn to_axum(self) -> AxumMessage;
}

impl Axumable for TungsteniteMessage {
    fn to_axum(self) -> AxumMessage {
        match self {
            TungsteniteMessage::Text(t) => AxumMessage::Text(t),
            TungsteniteMessage::Binary(b) => AxumMessage::Binary(b),
            TungsteniteMessage::Ping(p) => AxumMessage::Ping(p),
            TungsteniteMessage::Pong(p) => AxumMessage::Pong(p),
            TungsteniteMessage::Close(c) => {
                let close_message = c.map(|c| axum::extract::ws::CloseFrame {
                    reason: c.reason,
                    code: tungstenite_close_code_to_axum(c.code),
                });
                AxumMessage::Close(close_message)
            }
            TungsteniteMessage::Frame(_f) => todo!(),
        }
    }
}

trait Tungsteniteable {
    fn to_tungstenite(self) -> TungsteniteMessage;
}

impl Tungsteniteable for AxumMessage {
    fn to_tungstenite(self) -> TungsteniteMessage {
        match self {
            AxumMessage::Text(t) => TungsteniteMessage::Text(t),
            AxumMessage::Binary(b) => TungsteniteMessage::Binary(b),
            AxumMessage::Ping(p) => TungsteniteMessage::Ping(p),
            AxumMessage::Pong(p) => TungsteniteMessage::Pong(p),
            AxumMessage::Close(c) => {
                let close_message =
                    c.map(
                        |c| tokio_tungstenite::tungstenite::protocol::frame::CloseFrame {
                            reason: c.reason,
                            code: axum_close_code_to_tungstenite(c.code),
                        },
                    );
                TungsteniteMessage::Close(close_message)
            }
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
                        .send(msg.to_axum())
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
                        .send(msg.to_tungstenite())
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
    use super::*;
    use crate::*;
    use axum::{routing::get, Extension, Router};

    use futures_util::{SinkExt, StreamExt};

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
            inner: Arc::new(InnerStranglerService::new(
                axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port))
                    .unwrap(),
                SchemeSecurity::None,
                client,
            )),
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

    #[derive(Clone)]
    struct StopChannel(Arc<tokio::sync::broadcast::Sender<()>>);

    #[tokio::test]
    async fn proxies_strangled_websocket_service() {
        // helper function
        async fn handle_socket(
            mut socket: axum::extract::ws::WebSocket,
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

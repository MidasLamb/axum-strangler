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

use tower_service::Service;

mod builder;
mod inner;

pub enum HttpScheme {
    HTTP,
    #[cfg(feature = "https")]
    HTTPS,
}

#[cfg(feature = "websocket")]
pub enum WebSocketScheme {
    WS,
    #[cfg(any(
        feature = "websocket-native-tls",
        feature = "websocket-rustls-tls-native-roots",
        feature = "websocket-rustls-tls-webpki-roots"
    ))]
    WSS,
}

/// Service that forwards all requests to another service
/// ```rust
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let strangler_svc = axum_strangler::StranglerService::new(
///         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
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
    inner: Arc<dyn inner::InnerStrangler + Send + Sync>,
}

impl StranglerService {
    pub fn new(strangled_authority: axum::http::uri::Authority) -> Self {
        StranglerService::builder(strangled_authority).build()
    }

    pub fn builder(strangled_authority: axum::http::uri::Authority) -> builder::StranglerBuilder {
        builder::StranglerBuilder::new(strangled_authority)
    }
}

impl Service<axum::http::Request<axum::body::Body>> for StranglerService {
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let inner = self.inner.clone();

        let fut = async move { Ok(inner.forward_call_to_strangled(req).await) };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use axum::{routing::get, Extension, Router};

    /// Create a mock service that's not connecting to anything.
    fn make_svc() -> StranglerService {
        StranglerService::new(axum::http::uri::Authority::from_static("127.0.0.1:0"))
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

        let strangler_svc = StranglerService::new(
            axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port)).unwrap(),
        );

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
}

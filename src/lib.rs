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

use axum::{body::HttpBody, http::Uri};
use tower_service::Service;

/// Service that forwards all requests to another service
/// ```rust
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let http_client = reqwest::Client::new();
///     let strangler_svc = axum_strangler::StranglerService::new(
///         http_client,
///         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
///         axum::http::uri::Scheme::HTTP,
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
    http_client: reqwest::Client,
    inner: Arc<InnerStranglerService>,
}

impl StranglerService {
    /// Construct a new `StranglerService`.
    /// * The `strangled_authority` is the host & port of the service to be strangled.
    /// * The `strangled_scheme` is which scheme the strangled service has to make
    pub fn new(
        http_client: reqwest::Client,
        strangled_authority: axum::http::uri::Authority,
        strangled_scheme: axum::http::uri::Scheme,
    ) -> Self {
        Self {
            http_client,
            inner: Arc::new(InnerStranglerService {
                strangled_authority,
                strangled_scheme,
            }),
        }
    }
}

struct InnerStranglerService {
    strangled_authority: axum::http::uri::Authority,
    strangled_scheme: axum::http::uri::Scheme,
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

        let uri = Uri::builder()
            .authority(self.inner.strangled_authority.clone())
            .scheme(self.inner.strangled_scheme.clone())
            .path_and_query(req.uri().path_and_query().cloned().unwrap())
            .build()
            .unwrap();

        // TODO: set up headers
        let request_to_strangled = http_client.request(req.method().clone(), &uri.to_string());

        let fut = async move {
            let r = {
                request_to_strangled
                    .send()
                    .await
                    .map_err(|_e| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                    .unwrap()
            };

            let mut response_builder = axum::response::Response::builder();
            response_builder = response_builder.status(r.status());

            let body = r
                .bytes()
                .await
                .map_err(|_e| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                .unwrap();

            // TODO: map the headers.

            let response_body = axum::body::BoxBody::new(
                axum::body::Full::new(body)
                    .map_err(|_e| axum::Error::new("Making the typesystem happy")),
            );

            let response = response_builder
                .body(response_body)
                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR);

            // TODO: map Result<,x> to Result<,Infallible> by converting the the response to be a
            // 500 with an empty body.
            match response {
                Ok(response) => Ok(response),
                Err(_) => todo!(),
            }
        };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use axum::{routing::get, Router};

    /// Create a mock service that's not connecting to anything.
    fn make_svc() -> StranglerService {
        let client = reqwest::Client::new();
        let strangler_svc = StranglerService::new(
            client,
            axum::http::uri::Authority::from_static("127.0.0.1:0"),
            axum::http::uri::Scheme::HTTP,
        );
        strangler_svc
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

    #[tokio::test]
    async fn proxies_strangled_service() {
        let (tx, mut rx_1) = tokio::sync::broadcast::channel::<()>(1);
        let mut rx_2 = tx.subscribe();
        let tx_arc = Arc::new(tx);

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let client = reqwest::Client::new();
        let strangler_svc = StranglerService {
            http_client: client,
            inner: Arc::new(InnerStranglerService {
                strangled_authority: axum::http::uri::Authority::try_from(format!(
                    "127.0.0.1:{}",
                    stranglee_port
                ))
                .unwrap(),
                strangled_scheme: axum::http::uri::Scheme::HTTP,
            }),
        };

        let background_stranglee_handle = tokio::spawn(async move {
            let router = Router::new().route(
                "/api/something",
                get(|| async move {
                    tx_arc.send(()).unwrap();
                    "I'm being strangled"
                }),
            );

            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(router.into_make_service())
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

        background_stranglee_handle.await.unwrap();
        background_strangler_handle.await.unwrap();
    }
}

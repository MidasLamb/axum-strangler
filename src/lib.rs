//! # Axum Strangler
//! A `tower_service::Service` for use in the `axum` web framework to apply the  Strangler Fig pattern.
//! This makes "strangling" a bit easier, as everything that is handled by the "strangler" will
//! automatically no longer be forwarded to the "stranglee" or "strangled application" (a.k.a. the old application).
//!
//! ## Example
//! ```rust
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let strangler = axum_strangler::Strangler::new(
//!         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
//!     );
//!     let router = axum::Router::new().fallback_service(strangler);
//!     axum::Server::bind(&"127.0.0.1:0".parse()?)
//!         .serve(router.into_make_service())
//!         # .with_graceful_shutdown(async {
//!         # // Shut down immediately
//!         # })
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Caveats
//! Note that when registering a route with `axum`, all requests will be handled by it, even if you don't register anything for the specific method.
//! This means that in the following snippet, requests for `/new` with the method
//! POST, PUT, DELETE, OPTIONS, HEAD, PATCH, or TRACE will no longer be forwarded to the strangled application:
//! ```rust
//! async fn handler() {}
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let strangler = axum_strangler::Strangler::new(
//!         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
//!     );
//!     let router = axum::Router::new()
//!         .route(
//!             "/test",
//!              axum::routing::get(handler)
//!         )
//!         .fallback_service(strangler);
//!     axum::Server::bind(&"127.0.0.1:0".parse()?)
//!         .serve(router.into_make_service())
//!         # .with_graceful_shutdown(async {
//!         # // Shut down immediately
//!         # })
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! If you only want to implement a single method and still forward the rest, you can do so by adding the strangler as the fallback
//! for that specific `MethodRouter`:
//! ```rust
//! async fn handler() {}
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let strangler = axum_strangler::Strangler::new(
//!         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
//!     );
//!     let router = axum::Router::new()
//!         .route(
//!             "/test",
//!             axum::routing::get(handler)
//!                 .fallback_service(strangler.clone())
//!         )
//!         .fallback_service(strangler);
//!     axum::Server::bind(&"127.0.0.1:0".parse()?)
//!         .serve(router.into_make_service())
//!         # .with_graceful_shutdown(async {
//!         # // Shut down immediately
//!         # })
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Websocket support
//! If you enable the feature `websocket` (and possibly one of the supporting tls ones: websocket-native-tls,
//! websocket-rustls-tls-native-roots, websocket-rustls-tls-webpki-roots), a websocket will be set up, and each websocket
//! message will be relayed.
//!
//! ## Tracing propagation
//! Enabling the `tracing-opentelemetry-text-map-propagation` feature, will cause traceparent header to be set on
//! requests that get forwarded, based on the current `tracing` (& `tracing-opentelemetry`) context.
//!
//! Note that this requires the `opentelemetry` `TextMapPropagator` to be installed.

#![cfg_attr(docsrs, feature(doc_cfg))]

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
    #[cfg(any(docsrs, feature = "https"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "https")))]
    HTTPS,
}

#[cfg(any(docsrs, feature = "websocket"))]
#[cfg_attr(docsrs, doc(cfg(feature = "websocket")))]
pub enum WebSocketScheme {
    WS,
    #[cfg(any(
        feature = "websocket-native-tls",
        feature = "websocket-rustls-tls-native-roots",
        feature = "websocket-rustls-tls-webpki-roots"
    ))]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "websocket-native-tls",
            feature = "websocket-rustls-tls-native-roots",
            feature = "websocket-rustls-tls-webpki-roots"
        )))
    )]
    WSS,
}

/// Forwards all requests to another application.
/// Can be used in a lot of places, but the most common one would be as a `.fallback` on an `axum` `Router`.
/// # Example
/// ```rust
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let strangler_svc = axum_strangler::Strangler::new(
///         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
///     );
///     let router = axum::Router::new().fallback_service(strangler_svc);
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
pub struct Strangler {
    inner: Arc<dyn inner::InnerStrangler + Send + Sync>,
}

impl Strangler {
    /// Creates a new `Strangler` with the default options.
    /// For more control, see [`builder::StranglerBuilder`]
    pub fn new(strangled_authority: http::uri::Authority) -> Self {
        Strangler::builder(strangled_authority).build()
    }

    pub fn builder(strangled_authority: http::uri::Authority) -> builder::StranglerBuilder {
        builder::StranglerBuilder::new(strangled_authority)
    }

    /// Forwards the request to the strangled service.
    /// Meant to be used when you want to send something to the strangled application
    /// based on some custom logic.
    pub async fn forward_to_strangled(
        &self,
        req: http::Request<hyper::body::Body>,
    ) -> axum_core::response::Response {
        self.inner.forward_call_to_strangled(req).await
    }
}

impl Service<http::Request<hyper::body::Body>> for Strangler {
    type Response = axum_core::response::Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<hyper::body::Body>) -> Self::Future {
        let inner = self.inner.clone();

        let fut = async move { Ok(inner.forward_call_to_strangled(req).await) };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::HttpBody, Router};
    use wiremock::{
        matchers::{method, path},
        Mock, ResponseTemplate,
    };

    /// Create a mock service that's not connecting to anything.
    fn make_svc() -> Strangler {
        Strangler::new(axum::http::uri::Authority::from_static("127.0.0.1:0"))
    }

    #[tokio::test]
    async fn can_be_used_as_fallback() {
        let router = Router::new().fallback_service(make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[tokio::test]
    async fn can_be_used_for_a_route() {
        let router = Router::new().route_service("/api", make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[tokio::test]
    async fn proxies_strangled_http_service() {
        let mock_server = wiremock::MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/something"))
            .respond_with(ResponseTemplate::new(200).set_body_string("I'm being strangled"))
            .mount(&mock_server)
            .await;

        let strangler_svc = Strangler::new(
            axum::http::uri::Authority::try_from(format!(
                "127.0.0.1:{}",
                mock_server.address().port()
            ))
            .unwrap(),
        );

        let router = Router::new().fallback_service(strangler_svc);

        let req = http::Request::get("/api/something")
            .body(hyper::body::Body::empty())
            .unwrap();
        let mut res = router.clone().call(req).await.unwrap();

        assert_eq!(res.status(), http::StatusCode::OK);

        assert_eq!(
            res.body_mut().data().await.unwrap().unwrap(),
            "I'm being strangled".as_bytes()
        );
    }

    #[cfg(feature = "nested-routers")]
    #[tokio::test]
    async fn handles_nested_routers() {
        let mock_server = wiremock::MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/something"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/something-else"))
            .respond_with(ResponseTemplate::new(418))
            .mount(&mock_server)
            .await;

        let strangler_svc = Strangler::new(
            axum::http::uri::Authority::try_from(format!(
                "127.0.0.1:{}",
                mock_server.address().port()
            ))
            .unwrap(),
        );

        let nested_router = Router::new()
            .route_service("/something", strangler_svc.clone())
            .route_service("/something-else", strangler_svc.clone());
        let router = Router::new()
            .nest("/api", nested_router)
            .fallback_service(strangler_svc);

        let req = http::Request::get("/api/something")
            .body(hyper::body::Body::empty())
            .unwrap();
        let res = router.clone().call(req).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::OK);

        let req = http::Request::get("/api/something-else")
            .body(hyper::body::Body::empty())
            .unwrap();
        let res = router.clone().call(req).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::IM_A_TEAPOT);

        let req = http::Request::get("/not-api/something-else")
            .body(hyper::body::Body::empty())
            .unwrap();
        let res = router.clone().call(req).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::NOT_FOUND);
    }
}

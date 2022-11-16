use std::sync::Arc;

#[cfg(feature = "websocket")]
use crate::WebSocketScheme;
use crate::{
    inner::{InnerStrangler, InnerStranglerService},
    HttpScheme, StranglerService,
};

pub struct StranglerBuilder {
    authority: axum::http::uri::Authority,
    http_scheme: HttpScheme,
    #[cfg(feature = "websocket")]
    web_socket_scheme: WebSocketScheme,
    rewrite_strangled_request_host_header: bool,
}

impl StranglerBuilder {
    pub fn new(authority: axum::http::uri::Authority) -> Self {
        Self {
            authority,
            http_scheme: HttpScheme::HTTP,
            #[cfg(feature = "websocket")]
            web_socket_scheme: WebSocketScheme::WS,
            rewrite_strangled_request_host_header: false,
        }
    }

    /// The default is `HttpScheme::HTTP`
    pub fn with_http_scheme(self, http_scheme: HttpScheme) -> Self {
        Self {
            http_scheme,
            ..self
        }
    }

    /// The default is `WebSocketScheme::WS`
    #[cfg(feature = "websocket")]
    pub fn with_web_socket_scheme(self, web_socket_scheme: WebSocketScheme) -> Self {
        Self {
            web_socket_scheme,
            ..self
        }
    }

    /// Whether or not the service should rewrite the `host` header to the strangled target, or leave it be as is.
    /// If the other service is behind e.g. a `traefik` ingress controller or `nginx`, you probably want to set this
    /// to `true`.
    pub fn rewrite_strangled_request_host_header(
        self,
        rewrite_strangled_request_host_header: bool,
    ) -> Self {
        Self {
            rewrite_strangled_request_host_header,
            ..self
        }
    }

    pub fn build(self) -> StranglerService {
        let inner: Arc<dyn InnerStrangler + Send + Sync> = match self.http_scheme {
            HttpScheme::HTTP => {
                let inner = InnerStranglerService::new(
                    self.authority,
                    self.http_scheme,
                    #[cfg(feature = "websocket")]
                    self.web_socket_scheme,
                    hyper::Client::new(),
                    self.rewrite_strangled_request_host_header,
                );
                Arc::new(inner)
            }
            #[cfg(feature = "https")]
            HttpScheme::HTTPS => {
                let https = hyper_tls::HttpsConnector::new();
                let client = hyper::Client::builder().build::<_, hyper::Body>(https);
                let inner = InnerStranglerService::new(
                    self.authority,
                    self.http_scheme,
                    #[cfg(feature = "websocket")]
                    self.web_socket_scheme,
                    client,
                    self.rewrite_strangled_request_host_header,
                );
                Arc::new(inner)
            }
        };

        StranglerService { inner }
    }
}

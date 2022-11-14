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
}

impl StranglerBuilder {
    pub fn new(authority: axum::http::uri::Authority) -> Self {
        Self {
            authority,
            http_scheme: HttpScheme::HTTP,
            #[cfg(feature = "websocket")]
            web_socket_scheme: WebSocketScheme::WS,
        }
    }

    pub fn with_http_scheme(self, http_scheme: HttpScheme) -> Self {
        Self {
            http_scheme,
            ..self
        }
    }

    #[cfg(feature = "websocket")]
    pub fn with_web_socket_scheme(self, web_socket_scheme: WebSocketScheme) -> Self {
        Self {
            web_socket_scheme,
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
                );
                Arc::new(inner)
            }
        };

        StranglerService { inner }
    }
}

use axum::http::Uri;

use crate::HttpScheme;

#[cfg(feature = "websocket")]
use crate::WebSocketScheme;

#[cfg(feature = "websocket")]
mod websocket;

#[axum::async_trait]
pub(crate) trait InnerStrangler {
    async fn forward_call_to_strangled(
        &self,
        req: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response;
}

#[axum::async_trait]
impl<C> InnerStrangler for InnerStranglerService<C>
where
    C: hyper::client::connect::Connect + Clone + Send + Sync + 'static,
{
    async fn forward_call_to_strangled(
        &self,
        req: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response {
        let mut req = match self.handle_websocket_upgrade_request(req).await {
            Ok(r) => {
                return r;
            }
            Err(r) => r,
        };

        let strangled_authority = self.strangled_authority.clone();
        let strangled_scheme = self.get_http_scheme();
        let uri = Uri::builder()
            .scheme(strangled_scheme)
            .authority(strangled_authority)
            .path_and_query(req.uri().path_and_query().cloned().unwrap())
            .build()
            .unwrap();

        // Change the host header so any sort of other proxy in between can route correctly based on
        // hostname.
        if let Some(host) = req.headers_mut().get_mut("host") {
            *host = axum::http::HeaderValue::from_str(uri.authority().unwrap().as_str()).unwrap()
        }

        *req.uri_mut() = uri;

        let r = self.http_client.request(req).await.unwrap();

        let mut response_builder = axum::response::Response::builder();
        response_builder = response_builder.status(r.status());

        if let Some(headers) = response_builder.headers_mut() {
            *headers = r.headers().clone();
        }

        let response = response_builder
            .body(axum::body::boxed(r))
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR);

        match response {
            Ok(response) => response,
            Err(_) => todo!(),
        }
    }
}

pub(crate) struct InnerStranglerService<C> {
    strangled_authority: axum::http::uri::Authority,
    strangled_http_scheme: HttpScheme,
    #[cfg(feature = "websocket")]
    strangled_web_socket_scheme: WebSocketScheme,
    http_client: hyper::Client<C>,
}

impl<C> InnerStranglerService<C>
where
    C: hyper::client::connect::Connect + Clone + Send + Sync + 'static,
{
    pub(crate) fn new(
        strangled_authority: axum::http::uri::Authority,
        strangled_http_scheme: HttpScheme,
        #[cfg(feature = "websocket")] strangled_web_socket_scheme: WebSocketScheme,
        http_client: hyper::Client<C>,
    ) -> Self {
        Self {
            strangled_authority,
            strangled_http_scheme,
            #[cfg(feature = "websocket")]
            strangled_web_socket_scheme,
            http_client,
        }
    }

    #[cfg(not(feature = "websocket"))]
    async fn handle_websocket_upgrade_request(
        &self,
        req: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, axum::http::Request<axum::body::Body>> {
        Err(req)
    }

    fn get_http_scheme(&self) -> axum::http::uri::Scheme {
        match self.strangled_http_scheme {
            HttpScheme::HTTP => axum::http::uri::Scheme::HTTP,
            #[cfg(feature = "https")]
            HttpScheme::HTTPS => axum::http::uri::Scheme::HTTPS,
        }
    }
}

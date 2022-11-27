use std::str::FromStr;

use http::HeaderValue;
use opentelemetry::propagation::Injector;
use tracing_opentelemetry::OpenTelemetrySpanExt;

type Request = http::Request<hyper::body::Body>;

pub fn inject_opentelemetry_context_into_request(mut request: Request) -> Request {
    let context = tracing::Span::current().context();

    opentelemetry::global::get_text_map_propagator(|injector| {
        injector.inject_context(&context, &mut RequestCarrier::new(&mut request))
    });

    request
}

// Adapted from reqwest-middleware::otel
struct RequestCarrier<'a> {
    request: &'a mut Request,
}

impl<'a> RequestCarrier<'a> {
    pub fn new(request: &'a mut Request) -> Self {
        RequestCarrier { request }
    }
}

impl<'a> Injector for RequestCarrier<'a> {
    fn set(&mut self, key: &str, value: String) {
        let header_name = hyper::header::HeaderName::from_str(key).expect("Must be header name");
        let header_value = HeaderValue::from_str(&value).expect("Must be a header value");
        self.request.headers_mut().insert(header_name, header_value);
    }
}

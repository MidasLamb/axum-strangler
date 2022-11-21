# Axum Strangler

A utility crate to be able to easily use the Strangler Fig pattern with the Axum crate without having to use some sort of gateway.

## Goal of the crate

To support the usecase where you want to rewrite services in Rust, but you can't justify the
cost of migrating everything over all at once.
With the `Strangler`, you can put the Rustified service in front of the service you want
to migrate, and out of the box, almost everything should still work.
While migrating you slowly add more logic/routes to the Rust service and automatically those routes
won't be handled by the service you're migrating away from.

## Example

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // We want to forward requests we don't handle ourselves to localhost:3333
    let strangler_svc = axum_strangler::Strangler::new(
        axum::http::uri::Authority::from_static("127.0.0.1:3333"),
    );

    // Add it as a fallback so everything that isn't covered by routes, get forwarded to the strangled service.
    let router = axum::Router::new()
        .route("/hello",  get(|| async { "Hello, World!" }))
        .fallback(strangler_svc);

    axum::Server::bind(&"127.0.0.1:0".parse()?)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}
```

## Feature flags

### `https`

Allows forwarding to another server that only accepts HTTPS traffic:

```rust
let strangler_svc = axum_strangler::StranglerService::builder(
    axum::http::uri::Authority::from_static("127.0.0.1:3333"),
).with_http_scheme(axum_strangler::HttpScheme::HTTPS).build();
```

### `websocket`

Allows the strangler service to also handle websockets, forwarding every message from requester to strangled service and vice versa.

#### TLS

In order to work with websockets over TLS (`wss://`), you'll need to enable additional features.
You can choose which `tokio-tungstenite` dependency you use for tls, all of the three following features map on the counterpart there, but all three enable the `wss://` protocol:

- `websocket-native-tls`
- `websocket-rustls-tls-native-roots`
- `websocket-rustls-tls-webpki-roots`

### `tracing-opentelemetry-text-map-propagation`

Causes the Strangler to propagate tracing information to the stranglee. This could be useful to gather information about what exactly is going on.
This only works if there is an active `opentelemetry` context in the current `tracing` span, and you've installed the `opentelemetry::sdk::propagation::TraceContextPropagator` as the `opentelemetry::global::set_text_map_propagator`.

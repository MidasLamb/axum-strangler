use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    routing::get,
    Router,
};
use axum_strangler::{SchemeSecurity, StranglerService};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ACTIVE),
        )
        .with(tracing_subscriber::EnvFilter::new(
            "warn,axum_strangler=trace",
        ))
        .init();

    let stranglee_tcp = std::net::TcpListener::bind(format!(
        "127.0.0.1:{}",
        std::env::var("STRANGLEE_PORT").unwrap_or("0".to_owned())
    ))?;
    let stranglee_port = stranglee_tcp.local_addr()?.port();

    println!("Stranglee is listening on port {}", stranglee_port);

    let strangler_tcp = std::net::TcpListener::bind(format!(
        "127.0.0.1:{}",
        std::env::var("STRANGLER_PORT").unwrap_or("0".to_owned())
    ))?;
    let strangler_port = strangler_tcp.local_addr()?.port();

    println!("Strangler is listening on port {}", strangler_port);

    let strangler_svc = StranglerService::new(
        axum::http::uri::Authority::try_from(format!("127.0.0.1:{}", stranglee_port))?,
        SchemeSecurity::None,
    );

    tokio::spawn(async move {
        let router = Router::new()
            .route("/api/text", get(|| async move { "I'm being strangled" }))
            .route(
                "/api/json",
                get(|| async move { axum::response::Json("A json string") }),
            )
            .route(
                "/api/websocket",
                get(|ws: WebSocketUpgrade| async move { ws.on_upgrade(handle_socket) }),
            );

        axum::Server::from_tcp(stranglee_tcp)
            .unwrap()
            .serve(router.into_make_service())
            .await
            .unwrap();
    });

    let router = Router::new().fallback(strangler_svc);
    axum::Server::from_tcp(strangler_tcp)?
        .serve(router.into_make_service())
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C signal handler");
        })
        .await?;

    Ok(())
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        let msg = if let Ok(msg) = msg {
            msg
        } else {
            // client disconnected
            return;
        };

        if socket.send(msg).await.is_err() {
            // client disconnected
            return;
        }
    }
}

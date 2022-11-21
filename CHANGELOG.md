# Changelog

## 0.4.0-rc.2

- Add a function to allow a user to also easily forward requests, if they want to use it behind their own logic on whether or not to forward it.

## 0.4.0-rc.1

- Make it optional to rewrite the `host` header to the strangled host.
- Https & Wss support has been put behind the appropriate feature flags ([#12](https://github.com/MidasLamb/axum-strangler/pull/12))
- `tokio` dependency has been made optional ([#10](https://github.com/MidasLamb/axum-strangler/pull/10)), thanks to @syphar.
- `websocket` support has been put behind a feature flag.
- Remove unused `reqwest` dependency ([#7](https://github.com/MidasLamb/axum-strangler/pull/7)), thanks to @syphar.

## 0.3.1

- Add support for HTTPS
- Rewrite HOST header such that proxies down the line can correctly forward it (i.e. tls termination)

## 0.3.0

- Support for websocket strangling.
- Added a bit of tracing.

## 0.2.0

- Use `hyper::Client` instead of `reqwest::Client`
- Pass along the headers

## 0.1.0

- Initial release

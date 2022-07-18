# Axum Strangler
A utility crate to be able to easily use the Strangler Fig pattern with the Axum crate without having to use some sort of gateway.

## Goal of the crate
To support the usecase where you want to rewrite services in Rust, but you can't justify the
cost of migrating everything over all at once.
With the `StranglerService`, you can put the Rustified service in front of the service you want
to migrate, and out of the box, almost everything should still work. 
While migrating you slowly add more logic/routes to the Rust service and automatically those routes 
won't be handled by the service you're migrating away from.

## Roadmap
- [ ] Use `hyper::client` instead of `reqwest`, to more nicely integrate with `axum`.
- [ ] Allow configuring some sort of middleware to be able to do some comparisons between the service you're strangling, and the Rust version, to automatically detect regressions etc.



//! wadd's integration tests, as one binary: one link instead of one per
//! file keeps a test build's memory and disk down. Each file is a module;
//! run one with `cargo test -p wadd --test it registry::`.

mod common;

mod api;
mod cloud;
mod devices;
mod display;
mod drives;
mod github;
mod launches;
mod layers;
mod packages;
mod proxy;
mod registry;
mod secrets;
mod sets;
mod signins;
mod streams;
mod tailnet;
mod view;

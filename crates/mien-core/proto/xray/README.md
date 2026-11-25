# Vendored Xray management protos

Source: [XTLS/Xray-core](https://github.com/XTLS/Xray-core), tag `v26.11.9`, MPL-2.0.

This directory contains only the protobuf messages needed by Mien's shared
engine bootstrap and management API: proxyman HandlerService, router
RoutingService, core handler envelopes, serial typed messages, protocol
endpoint/user messages, network/port primitives, and transport stream config.
The vendored files are intentionally kept as source inputs so CI can generate
clients without fetching Xray or its source tree.

`crates/mien-core/build.rs` uses `tonic-prost-build` and the vendored files to
generate a client-only Rust module in Cargo's `OUT_DIR`; `protoc-bin-vendored`
provides a deterministic compiler for offline builds. Credentials are encoded
only into in-memory gRPC request messages and are never written to these files.

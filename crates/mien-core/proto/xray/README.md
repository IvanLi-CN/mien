# Vendored Xray management protos

Source: [XTLS/Xray-core](https://github.com/XTLS/Xray-core), tag `v26.11.9`, MPL-2.0.

This directory contains only the protobuf messages needed by Mien's shared
engine bootstrap and management API: proxyman HandlerService, router
RoutingService, core handler envelopes, serial typed messages, protocol
endpoint/user messages, network/port primitives, and transport stream config.
The vendored files are intentionally kept as source inputs so CI can generate
clients without fetching Xray or its source tree.

Vendored protocol messages (`build.rs` compiles each file from this list):

| Path | Purpose |
| --- | --- |
| `app/proxyman/command/command.proto` | `AddInbound`/`AddOutbound`/`List*`/`Remove*` management service |
| `app/proxyman/config.proto` | receiver/sender, sniffing, multiplexing config |
| `app/router/command/command.proto` | `AddRule`/`ListRule`/`RemoveRule` service |
| `app/router/config.proto` | routing rules and rule targets |
| `common/net/*.proto` | IP or domain, port ranges, network enums |
| `common/protocol/*.proto` | server endpoint, user, VMess security enum |
| `common/serial/typed_message.proto` | typed protobuf envelope |
| `common/geodata/geodat.proto`, `common/log/log.proto` | transitive imports of core config |
| `core/config.proto` | core config envelopes |
| `transport/internet/config.proto` | stream/transport settings |
| `proxy/http/config.proto` | basic HTTP inbound/outbound encoding |
| `proxy/dokodemo/config.proto`, `proxy/blackhole/config.proto`, `proxy/freedom/config.proto` | bootstrap outbound/inbound messages |
| `proxy/vless/account.proto`, `proxy/vless/outbound/config.proto` | Tier A VLESS |
| `proxy/vmess/account.proto`, `proxy/vmess/outbound/config.proto` | Tier A VMess |
| `proxy/shadowsocks/config.proto` | Tier A Shadowsocks (`Account.CipherType` is the cipher enum) |
| `proxy/trojan/config.proto` | Tier A Trojan |

Every file is byte-identical to `XTLS/Xray-core` at tag `v26.11.9`; packages and
directory layout are unchanged so the generated types keep their upstream names.

`crates/mien-core/build.rs` uses `tonic-prost-build` and the vendored files to
generate a client-only Rust module in Cargo's `OUT_DIR`; `protoc-bin-vendored`
provides a deterministic compiler for offline builds. Credentials are encoded
only into in-memory gRPC request messages and are never written to these files.

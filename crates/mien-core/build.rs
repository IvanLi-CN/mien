fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("proto/xray");
    let files = [
        "app/proxyman/command/command.proto",
        "app/router/command/command.proto",
        "common/net/address.proto",
        "common/net/network.proto",
        "common/net/port.proto",
        "common/protocol/headers.proto",
        "common/protocol/server_spec.proto",
        "common/protocol/user.proto",
        "common/geodata/geodat.proto",
        "common/log/log.proto",
        "common/serial/typed_message.proto",
        "core/config.proto",
        "app/proxyman/config.proto",
        "app/router/config.proto",
        "transport/internet/config.proto",
        "proxy/http/config.proto",
        "proxy/dokodemo/config.proto",
        "proxy/blackhole/config.proto",
        "proxy/freedom/config.proto",
        "proxy/vless/outbound/config.proto",
        "proxy/vless/account.proto",
        "proxy/vmess/outbound/config.proto",
        "proxy/vmess/account.proto",
        "proxy/shadowsocks/config.proto",
        "proxy/trojan/config.proto",
    ]
    .iter()
    .map(|p| root.join(p))
    .collect::<Vec<_>>();
    unsafe { std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?) };
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_protos(&files, std::slice::from_ref(&root))?;
    println!("cargo:rerun-if-changed=proto/xray");
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

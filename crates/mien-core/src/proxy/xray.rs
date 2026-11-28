//! Generated Xray protocol buffers.
pub mod app {
    pub mod proxyman {
        tonic::include_proto!("xray.app.proxyman");
        pub mod command {
            tonic::include_proto!("xray.app.proxyman.command");
        }
    }
    pub mod router {
        tonic::include_proto!("xray.app.router");
        pub mod command {
            tonic::include_proto!("xray.app.router.command");
        }
    }
}
pub mod common {
    pub mod net {
        tonic::include_proto!("xray.common.net");
    }
    pub mod protocol {
        tonic::include_proto!("xray.common.protocol");
    }
    pub mod serial {
        tonic::include_proto!("xray.common.serial");
    }
    pub mod geodata {
        tonic::include_proto!("xray.common.geodata");
    }
    pub mod log {
        tonic::include_proto!("xray.common.log");
    }
}
pub mod core {
    tonic::include_proto!("xray.core");
}
pub mod transport {
    pub mod internet {
        tonic::include_proto!("xray.transport.internet");
    }
}
pub mod proxy {
    pub mod http {
        tonic::include_proto!("xray.proxy.http");
    }
    pub mod dokodemo {
        tonic::include_proto!("xray.proxy.dokodemo");
    }
    pub mod blackhole {
        tonic::include_proto!("xray.proxy.blackhole");
    }
    pub mod freedom {
        tonic::include_proto!("xray.proxy.freedom");
    }
    pub mod vless {
        tonic::include_proto!("xray.proxy.vless");
        pub mod outbound {
            tonic::include_proto!("xray.proxy.vless.outbound");
        }
    }
    pub mod vmess {
        tonic::include_proto!("xray.proxy.vmess");
        pub mod outbound {
            tonic::include_proto!("xray.proxy.vmess.outbound");
        }
    }
    pub mod shadowsocks {
        tonic::include_proto!("xray.proxy.shadowsocks");
    }
    pub mod trojan {
        tonic::include_proto!("xray.proxy.trojan");
    }
}

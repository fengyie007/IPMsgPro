use ipmsg_core::{config::parse_address, protocol::subnet_broadcast};
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddrV4},
};

#[derive(Clone)]
pub struct Options {
    pub port: u16,
    pub direct: Vec<SocketAddrV4>,
    pub verbose: bool,
}
impl Options {
    pub fn parse() -> Result<Self, String> {
        let mut options = Self {
            port: 2427,
            direct: vec![],
            verbose: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            let (key, value) = argument
                .split_once('=')
                .map(|(k, v)| (k, Some(v.to_string())))
                .unwrap_or((&argument, None));
            match key {
                "--port" => {
                    options.port = value
                        .or_else(|| args.next())
                        .ok_or("--port缺少值")?
                        .parse::<u16>()
                        .map_err(|_| "无效端口")?;
                    if options.port == 0 {
                        return Err("端口不能为0".into());
                    }
                }
                "--adduser" => {
                    for item in value
                        .or_else(|| args.next())
                        .ok_or("--adduser缺少值")?
                        .split(',')
                    {
                        options.direct.push(parse_address(item)?);
                    }
                }
                "--verbose" => options.verbose = true,
                _ => {
                    return Err(format!(
                        "未知参数：{argument}；支持 --port=N --adduser=IPv4:port,... --verbose"
                    ))
                }
            }
        }
        Ok(options)
    }
}

// OS adapter data is read into an aligned buffer; no application data is modified.
#[cfg(windows)]
pub fn local_networks() -> (Ipv4Addr, Vec<Ipv4Addr>) {
    use windows_sys::Win32::{
        Foundation::ERROR_BUFFER_OVERFLOW,
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::{AF_INET, SOCKADDR_IN},
    };
    let mut size = 16384u32;
    let mut storage = vec![0u64; (size as usize + 7) / 8];
    let mut result;
    loop {
        result = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                storage.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != ERROR_BUFFER_OVERFLOW || size > 1024 * 1024 {
            break;
        }
        storage.resize((size as usize + 7) / 8, 0);
    }
    let mut addresses = vec![];
    let mut broadcasts = BTreeSet::new();
    if result == 0 {
        let mut adapter = storage.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        unsafe {
            while !adapter.is_null() {
                if (*adapter).OperStatus == IfOperStatusUp {
                    let mut unicast = (*adapter).FirstUnicastAddress;
                    while !unicast.is_null() {
                        let address = (*unicast).Address;
                        if !address.lpSockaddr.is_null()
                            && address.iSockaddrLength as usize
                                >= std::mem::size_of::<SOCKADDR_IN>()
                            && (*address.lpSockaddr).sa_family == AF_INET
                        {
                            let address = &*address.lpSockaddr.cast::<SOCKADDR_IN>();
                            let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.S_un.S_addr));
                            let prefix = (*unicast).OnLinkPrefixLength;
                            if !ip.is_loopback()
                                && !ip.is_unspecified()
                                && (1..=30).contains(&prefix)
                            {
                                addresses.push(ip);
                                if let Some(broadcast) = subnet_broadcast(ip, prefix) {
                                    broadcasts.insert(broadcast);
                                }
                            }
                        }
                        unicast = (*unicast).Next;
                    }
                }
                adapter = (*adapter).Next;
            }
        }
    }
    if broadcasts.is_empty() {
        broadcasts.insert(Ipv4Addr::BROADCAST);
    }
    (
        addresses.first().copied().unwrap_or(Ipv4Addr::LOCALHOST),
        broadcasts.into_iter().collect(),
    )
}
#[cfg(not(windows))]
pub fn local_networks() -> (Ipv4Addr, Vec<Ipv4Addr>) {
    (Ipv4Addr::LOCALHOST, vec![Ipv4Addr::BROADCAST])
}

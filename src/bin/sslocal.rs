use std::cell::RefCell;
use std::io;
use std::net::IpAddr;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use futures_util::FutureExt;
use shadowsocks::args::parse_args;
use shadowsocks::cipher::Cipher;
use shadowsocks::config::Config;
use shadowsocks::io::{copy_bidirectional, write_all};
use shadowsocks::socks5;
use shadowsocks::socks5::v5::{TYPE_DOMAIN, TYPE_IPV4, TYPE_IPV6};
use slings::net::{TcpListener, TcpStream};
use slings::runtime::Runtime;

fn main() -> io::Result<()> {
    env_logger::init();
    let config = parse_args("sslocal").unwrap();
    log::info!("{}", serde_json::to_string_pretty(&config).unwrap());
    let runtime = Runtime::new()?;

    runtime.block_on(async {
        let listener = TcpListener::bind(&config.local_addr).await?;
        log::info!("listening connections on {}", config.local_addr);

        let cipher = Cipher::new(&config.method, &config.password);
        let config = Rc::new(config);

        loop {
            let (socket, addr) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            log::debug!("accept tcp stream from addr {:?}", addr);
            let cipher = Rc::new(RefCell::new(cipher.reset()));
            let proxy = proxy(config.clone(), cipher, socket).map(|r| {
                if let Err(e) = r {
                    log::error!("failed to proxy; error={}", e);
                }
            });
            slings::spawn_local(proxy).detach();
        }
    })
}

async fn proxy(
    config: Rc<Config>,
    cipher: Rc<RefCell<Cipher>>,
    mut socket1: TcpStream,
) -> io::Result<(u64, u64)> {
    let (host, port) = socks5::handshake(&mut socket1, Duration::from_secs(3)).await?;

    log::debug!("proxy to address: {}:{}", host, port);

    let mut socket2 = TcpStream::connect(&config.server_addr).await?;
    log::debug!("connected to server {}", config.server_addr);

    let rawaddr = generate_raw_addr(&host, port);
    write_all(cipher.clone(), &mut socket2, &rawaddr).await?;

    let (n1, n2) = copy_bidirectional(&mut socket1, &mut socket2, cipher).await?;
    log::debug!("proxy local => remote: {}, remote => local: {:?}", n1, n2);
    Ok((n1, n2))
}

fn generate_raw_addr(host: &str, port: u16) -> Vec<u8> {
    match IpAddr::from_str(host) {
        Ok(IpAddr::V4(host)) => {
            let mut rawaddr = vec![TYPE_IPV4];
            rawaddr.extend_from_slice(&host.octets());
            rawaddr.extend_from_slice(&[((port >> 8) & 0xff) as u8, (port & 0xff) as u8]);
            rawaddr
        }
        Ok(IpAddr::V6(host)) => {
            let mut rawaddr = vec![TYPE_IPV6];
            rawaddr.extend_from_slice(&host.octets());
            rawaddr.extend_from_slice(&[((port >> 8) & 0xff) as u8, (port & 0xff) as u8]);
            rawaddr
        }
        _ => {
            let dm_len = host.as_bytes().len();
            let mut rawaddr = vec![TYPE_DOMAIN, dm_len as u8];
            rawaddr.extend_from_slice(host.as_bytes());
            rawaddr.extend_from_slice(&[((port >> 8) & 0xff) as u8, (port & 0xff) as u8]);
            rawaddr
        }
    }
}

extern crate tokio_core;
extern crate tokio_io;
extern crate futures;
use std::fmt;
use std::time::Duration;
use std::net::{TcpStream, SocketAddr};
use std::io::{self, Write, Read, BufReader, BufRead, ErrorKind};
use self::futures::{future, Future};
use self::tokio_core::reactor::Handle;
use self::tokio_core::net as tnet;
use self::tokio_io::io::{write_all, read_until};
use ::proxy::ProxyServer;


#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct HttpProxyServer {
    tag: String,
    addr: SocketAddr,
}

impl HttpProxyServer {
    pub fn new(addr: SocketAddr) -> HttpProxyServer {
        let tag = format!("{}", addr.port());
        HttpProxyServer {
            tag: tag,
            addr: addr,
        }
    }
}

impl fmt::Display for HttpProxyServer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} (HTTP {})", self.tag, self.addr)
    }
}

impl ProxyServer for HttpProxyServer {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn connect(&self, addr: SocketAddr) -> io::Result<TcpStream> {
        let mut stream = TcpStream::connect(self.addr)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        debug!("creating proxy tunnel to {} via {}", addr, self.tag());

        stream.write_all(build_request(&addr).as_bytes())?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if !line.starts_with("HTTP/1.1 2") {
            info!("{} return {}", self.tag(), line.trim());
            let err = format!("proxy server return error: {}", line.trim());
            return Err(io::Error::new(ErrorKind::Other, err));
        }
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        // FIXME: may lost data in buffer
        let stream = reader.into_inner();

        debug!("proxy tunnel connected");
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        Ok(stream)
    }

    fn connect_async(&self, addr: SocketAddr, handle: &Handle)
            -> Box<Future<Item=tnet::TcpStream, Error=io::Error>> {
        let conn = tnet::TcpStream::connect(&self.addr, handle);
        let request = conn.and_then(move |stream| {
            write_all(stream, build_request(&addr))
        });
        let response = request.and_then(|(stream, _)| {
            let reader = BufReader::new(stream).take(512);
            read_until(reader, 0x0a, Vec::with_capacity(32))
        }).and_then(|(reader, status)| {
            let status = match String::from_utf8(status) {
                Ok(s) => s,
                Err(e) => return future::err(io::Error::new(ErrorKind::Other,
                        format!("fail to parse http response: {}", e))),
            };
            if status.starts_with("HTTP/1.1 2") {
                future::ok(reader.into_inner())
            } else {
                let err = format!("proxy return error: {}", status.trim());
                future::err(io::Error::new(ErrorKind::Other, err))
            }
        });
        let skip = response.and_then(|reader| {
            let buf = Vec::with_capacity(64);
            future::loop_fn((reader, buf), |(reader, mut buf)| {
                buf.clear();
                let reader = reader.take(2048);
                read_until(reader, 0x0a, buf).and_then(|(reader, buf)| {
                    let reader = reader.into_inner();
                    if buf.len() <= 2 {
                        Ok(future::Loop::Break(reader))
                    } else {
                        Ok(future::Loop::Continue((reader, buf)))
                    }
                })
            })
        });
        // FIXME: may lost data in buffer
        Box::new(skip.map(|reader| reader.into_inner()))
    }
}

fn build_request(addr: &SocketAddr) -> String {
        let host = match *addr {
            SocketAddr::V4(s) => format!("{}:{}", s.ip(), s.port()),
            SocketAddr::V6(s) => format!("[{}]:{}", s.ip(), s.port()),
        };
        let request = format!(
            "CONNECT {host} HTTP/1.1\r\n\
            Host: {host}\r\n\
            Connection: close\r\n\r\n",
            host=host
        );
        return request;
}

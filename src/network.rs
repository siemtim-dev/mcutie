
use embassy_futures::select::select;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embedded_io_async::Write;
use embedded_io_async::Read;


/// Error type for network
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    
    /// The DNS Request failed
    #[error("The DNS Lookup failed")]
    DnsError,

    /// The Host could not be found
    #[error("The host was not found")]
    HostNotFound,

    /// The cpnnection could not be created
    #[error("tcp connect failed")]
    ConnectionFailed

}

pub trait TcpConnection<'a>: Read + Write + 'a {
    fn split(&mut self) -> (impl Read, impl Write);

    fn close(&mut self);
}

impl <'a> TcpConnection<'a> for embassy_net::tcp::TcpSocket<'a> {
    
    fn split(&mut self) -> (impl Read, impl Write) {
        embassy_net::tcp::TcpSocket::split(self)
    }

    fn close(&mut self) {
        embassy_net::tcp::TcpSocket::close(self)
    }
}

/// The `Network` trait is an abstraction over a specific network stack.
pub trait Network<'a> {

    /// Resolves if the underlying network stack is ready to use.
    /// Can return a `NetworkError` if there is son´mething wrong.
    /// Must be called before using the network stack
    async fn ready(&self) -> Result<(), NetworkError>;


    /// Create a TCP connection to the specified host
    async fn connect(&self, host: &str, port: u16, rx_buffer: &'a mut [u8], tx_buffer: &'a mut [u8]) 
        -> Result<impl TcpConnection<'a>, NetworkError>;

    /// Resolves when there is a network problem
    async fn net_down(&self);
    
}

/// Network implementation using crate `embassy_net`
pub struct EmbassyNetwork<'d>(Stack<'d>);


impl <'d> EmbassyNetwork<'d> {
    /// Create a new Network stack
    pub fn new(stack: Stack<'d>) -> Self {
        Self(stack)
    }
}


impl <'a> Network<'a> for EmbassyNetwork<'a> {
    
    
    async fn ready(&self) -> Result<(), NetworkError> {
        if ! self.0.is_config_up() {
            debug!("Waiting for network to configure.");
            self.0.wait_config_up().await;
            debug!("Network configured.");
        }
        Ok(())
    }

    async fn connect(&self, host: &str, port: u16, rx_buffer: &'a mut [u8], tx_buffer: &'a mut [u8]) 
        -> Result<impl TcpConnection<'a>, NetworkError> {
        

        let socket = embassy_net_connect(self.0, host, port, rx_buffer, tx_buffer).await?;

        Ok(socket)
    }

    /// Resolves when there is a network problem
    async fn net_down(&self) {
        let link_down = async {
            self.0.wait_link_down().await;
            warn!("Network link lost");
        };

        let ip_down = async {
            self.0.wait_config_down().await;
            warn!("Network config lost");
        };

        select(link_down, ip_down).await;
    }
}

async fn embassy_net_connect<'a>(stack: Stack<'a>, host: &str, port: u16, rx_buffer: &'a mut [u8], tx_buffer: &'a mut [u8]) -> Result<TcpSocket<'a>, NetworkError> {
    let ip_addrs = stack.dns_query(host, DnsQueryType::A).await
        .map_err(|err| {
            error!("Failed to lookup '{}' for broker: {:?}", host, &err);
            NetworkError::DnsError
        })?;

    let ip = ip_addrs.first().ok_or_else(|| {
        error!("No IP address found for broker '{}'", host);
        NetworkError::HostNotFound
    })?;


    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    let endpoint = (*ip, port);

    socket.connect(endpoint).await.map_err(|e|{
        error!("Failed to connect to {}:{}: {:?}", ip, port, e);
        NetworkError::ConnectionFailed
    })?;

    Ok(socket)
}

#[cfg(test)]
pub(crate) mod std_network {



}

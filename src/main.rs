use anyhow::{anyhow, Result};
use byteorder::{BigEndian, ByteOrder};
use chrono::{DateTime, Utc};
use reqwest;
use std::sync::{Arc, atomic::{AtomicI64, Ordering}};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{interval, Duration};
use tracing::{info, warn};
use tracing_subscriber;

const NTP_EPOCH_OFFSET: i64 = 2_208_988_800; // NTP epoch is 1900, Unix epoch is 1970

#[derive(Clone)]
struct TimeCache {
    offset: Arc<AtomicI64>,
}

impl TimeCache {
    fn new(initial_time: DateTime<Utc>) -> Self {
        let system_now = Utc::now().timestamp();
        let ntp_now = initial_time.timestamp() + NTP_EPOCH_OFFSET;
        let offset = ntp_now - system_now;
        Self {
            offset: Arc::new(AtomicI64::new(offset)),
        }
    }

    fn get_ntp_timestamp(&self) -> u64 {
        let system_now = Utc::now().timestamp();
        let offset = self.offset.load(Ordering::Relaxed);
        (system_now + offset) as u64
    }

    fn update(&self, new_time: DateTime<Utc>) {
        let system_now = Utc::now().timestamp();
        let ntp_now = new_time.timestamp() + NTP_EPOCH_OFFSET;
        let offset = ntp_now - system_now;
        self.offset.store(offset, Ordering::Relaxed);
    }
}

async fn fetch_time(cache: &TimeCache, url: &str) -> Result<()> {
    let resp = reqwest::get(url).await?;
    let date_header = resp.headers().get("Date").ok_or(anyhow!("Missing Date header"))?;
    let date_str = date_header.to_str()?;
    let parsed_time = DateTime::parse_from_rfc2822(date_str)?.with_timezone(&Utc);

    cache.update(parsed_time);
    info!("Updated cached time to: {}", parsed_time);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let time_fetch_url = "https://jegy.mav.hu/";
    let time_fetch_interval = Duration::from_secs(24 * 3600);

    let initial_time = {
        let resp = reqwest::get(time_fetch_url).await?;
        let date_header = resp.headers().get("Date").ok_or(anyhow!("Date header missing"))?;
        let parsed_time = DateTime::parse_from_rfc2822(date_header.to_str()?)?.with_timezone(&Utc);
        parsed_time
    };

    let cache = Arc::new(TimeCache::new(initial_time));
    let cache_clone = Arc::clone(&cache);

    tokio::spawn(async move {
        let mut ticker = interval(time_fetch_interval);
        loop {
            ticker.tick().await;
            if let Err(e) = fetch_time(&cache_clone, time_fetch_url).await {
                warn!("Failed to update cached time: {}", e);
            }
        }
    });

    let addr = "0.0.0.0:123";
    let socket = Arc::new(UdpSocket::bind(addr).await?);
    info!("Listening on {}", addr);

    let mut buf = [0u8; 48];
    loop {
        let (len, remote_addr) = socket.recv_from(&mut buf).await?;
        if len < 48 {
            warn!("Received short packet from {}", remote_addr);
            continue;
        }

        let socket_clone = Arc::clone(&socket);
        let cache_clone = Arc::clone(&cache);
        let request_copy = buf;

        tokio::spawn(async move {
            handle_client(socket_clone, remote_addr, request_copy, cache_clone).await;
        });
    }
}

async fn handle_client(socket: Arc<UdpSocket>, remote_addr: SocketAddr, request: [u8; 48], cache: Arc<TimeCache>) {
    let ntp_timestamp = cache.get_ntp_timestamp();
    let mut response = [0u8; 48];

    response[0] = 0x1C; // LI=0, Version=4, Mode=4
    response[1] = 15;   // Stratum 15
    response[2] = 6;    // Poll interval
    response[3] = 0xEC; // Precision -20

    BigEndian::write_u32(&mut response[4..8], 1 << 16); // Root Delay
    BigEndian::write_u32(&mut response[8..12], 1 << 16); // Root Dispersion
    response[12] = 0x4D; // Reference ID: 'M'

    BigEndian::write_u32(&mut response[16..20], ntp_timestamp as u32);
    BigEndian::write_u32(&mut response[20..24], 0);
    response[24..32].copy_from_slice(&request[40..48]); // Origin Timestamp
    BigEndian::write_u32(&mut response[32..36], ntp_timestamp as u32);
    BigEndian::write_u32(&mut response[36..40], 0);
    BigEndian::write_u32(&mut response[40..44], ntp_timestamp as u32);
    BigEndian::write_u32(&mut response[44..48], 0);

    if let Err(e) = socket.send_to(&response, remote_addr).await {
        warn!("Error sending response: {}", e);
    }
}

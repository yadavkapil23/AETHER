use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use minifb::{Key, Window, WindowOptions};
use tokio::sync::mpsc;
use tracing::{info, Level};

use transport::metrics::TransportMetrics;
use transport::quic_path::QuicPath;
use transport::srt_lite::SrtReceiver;
use transport::tls::generate_self_signed_configs;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    info!("Starting AETHER Player...");

    // Channel to send frames from the async network tasks to the synchronous GUI thread
    let (frame_tx, mut frame_rx) = mpsc::channel::<Bytes>(1024);

    // Spawn Tokio runtime in a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let (server_config, _) = generate_self_signed_configs().unwrap();
            let metrics = Arc::new(TransportMetrics::default());

            // Listen for QUIC (keyframes)
            let quic_addr: SocketAddr = "0.0.0.0:4433".parse().unwrap();
            // remote_addr is unused in run_receiver, pass dummy
            let dummy_remote: SocketAddr = "127.0.0.1:0".parse().unwrap(); 
            let quic_path = QuicPath::new(quic_addr, dummy_remote, server_config, metrics).await.unwrap();
            
            let quic_tx = frame_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = quic_path.run_receiver(quic_tx).await {
                    tracing::error!("QUIC receiver error: {}", e);
                }
            });

            // Listen for SRT-lite (P-frames)
            let srt_addr: SocketAddr = "0.0.0.0:5000".parse().unwrap();
            let srt_path = SrtReceiver::new(srt_addr);
            
            let srt_tx = frame_tx;
            tokio::spawn(async move {
                if let Err(e) = srt_path.run(srt_tx).await {
                    tracing::error!("SRT receiver error: {}", e);
                }
            });

            // Keep the runtime alive
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    });

    // GUI Loop (must run on the main thread)
    let width = 640;
    let height = 360;
    let mut window = Window::new(
        "AETHER Receiver - Live Stream",
        width,
        height,
        WindowOptions::default(),
    )?;

    // We don't have a real H.264 decoder in this stub, so we will visualize
    // the incoming stream by generating a shifting color pattern based on
    // frame arrival, proving the transport works at 60fps.
    let mut buffer: Vec<u32> = vec![0; width * height];
    let mut frame_count = 0u32;
    let mut last_log = std::time::Instant::now();
    let mut bytes_received_sec = 0;

    window.set_target_fps(60);

    while window.is_open() && !window.is_key_down(Key::Escape) {
        // Drain all available frames from the network
        while let Ok(payload) = frame_rx.try_recv() {
            frame_count = frame_count.wrapping_add(1);
            bytes_received_sec += payload.len();
        }

        // Draw synthetic pattern to prove GUI update
        for y in 0..height {
            for x in 0..width {
                // Shifting color pattern driven by the network frame_count
                let r = ((x as u32 + frame_count * 2) % 255) as u8;
                let g = ((y as u32 + frame_count * 2) % 255) as u8;
                let b = 150u8;
                
                let pixel = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                buffer[y * width + x] = pixel;
            }
        }

        window.update_with_buffer(&buffer, width, height)?;

        if last_log.elapsed() >= Duration::from_secs(1) {
            info!("Received {} bytes in the last second, frame_count={}", bytes_received_sec, frame_count);
            bytes_received_sec = 0;
            last_log = std::time::Instant::now();
        }
    }

    Ok(())
}

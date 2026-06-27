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
use openh264::decoder::Decoder;
use proto::EncodedFrame; // Need EncodedFrame if that's what the transport gives, but the channel is Bytes!

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
    let mut width = 640;
    let mut height = 360;
    let mut window = Window::new(
        "AETHER Receiver - Live Stream",
        width,
        height,
        WindowOptions::default(),
    )?;

    window.set_target_fps(60);

    let api = openh264::OpenH264API::from_source();
    let mut decoder = Decoder::new(api).unwrap();
    
    // We will allocate these dynamically when the first frame arrives
    let mut buffer: Vec<u32> = vec![0; width * height];
    let mut rgb_buf: Vec<u8> = vec![0; width * height * 3];

    let mut frame_count = 0u32;
    let mut last_log = std::time::Instant::now();
    let mut bytes_received_sec = 0;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        let mut got_new_frame = false;
        
        // Drain all available frames from the network
        while let Ok(payload) = frame_rx.try_recv() {
            frame_count = frame_count.wrapping_add(1);
            bytes_received_sec += payload.len();
            
            // Actually decode the H.264 bitstream!
            match decoder.decode(&payload) {
                Ok(Some(yuv)) => {
                    let (w, h) = yuv.dimension_rgb();
                    
                    // Resize buffers if the stream resolution changes
                    if w != width || h != height {
                        width = w;
                        height = h;
                        buffer.resize(width * height, 0);
                        rgb_buf.resize(width * height * 3, 0);
                    }
                    
                    // OpenH264 can convert to RGB8 for us
                    yuv.write_rgb8(&mut rgb_buf);
                    
                    // Convert packed RGB to minifb's XRGB u32 format
                    for (i, pixel) in rgb_buf.chunks_exact(3).enumerate() {
                        let r = pixel[0] as u32;
                        let g = pixel[1] as u32;
                        let b = pixel[2] as u32;
                        buffer[i] = (r << 16) | (g << 8) | b;
                    }
                    
                    got_new_frame = true;
                }
                Ok(None) => {
                    // Decoder needs more data (e.g. part of a frame or waiting for IDR)
                }
                Err(e) => {
                    tracing::error!("Decode error: {}", e);
                }
            }
        }

        if got_new_frame {
            window.update_with_buffer(&buffer, width, height)?;
        } else {
            // Update window to keep it responsive if no frames arrive
            window.update();
        }

        if last_log.elapsed() >= Duration::from_secs(1) {
            info!("Received {} bytes in the last second, decoded {} frames", bytes_received_sec, frame_count);
            frame_count = 0; // reset for the log
            bytes_received_sec = 0;
            last_log = std::time::Instant::now();
        }
    }

    Ok(())
}


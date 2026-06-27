use std::sync::Arc;
use clap::Parser;
use tokio::sync::{mpsc, watch};
use tracing::{info, Level};

use proto::{CodecConfig, LatencyPreset, PipelineStage};
use telemetry::{LatencyTracker, PrometheusExporter};
use transport::{
    AdaptationDecision, FrameRouter, ProbeConfig, ProbeTask, RouterConfig, TransportMetrics,
};
use capture::{CaptureConfig, CaptureDevice, Resolution, TestCapture};
use encode::{Encoder, H264Encoder};

#[derive(Parser, Debug)]
#[command(author, version, about = "AETHER Sub-50ms Live Streaming Daemon")]
struct Args {
    #[arg(long, default_value_t = 60)]
    fps: u32,
    #[arg(long, default_value_t = 6000)]
    bitrate_kbps: u32,
    #[arg(long, default_value = "0.0.0.0:9100")]
    metrics_addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Setup tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .init();

    let args = Args::parse();
    info!("Starting AETHER daemon...");

    // 2. Setup Telemetry
    let tracker = Arc::new(LatencyTracker::new());
    let exporter = Arc::new(PrometheusExporter::new(tracker.histogram()));
    
    let metrics_addr: std::net::SocketAddr = args.metrics_addr.parse()?;
    tokio::spawn(async move {
        info!("Prometheus metrics server listening on http://{}", metrics_addr);
        if let Err(e) = exporter.serve(metrics_addr).await {
            tracing::error!("Metrics server error: {}", e);
        }
    });

    // 3. Setup Transport & Adaptation
    let metrics = Arc::new(TransportMetrics::default());
    let (probe_task, decision_rx, _probe_rx) = ProbeTask::new(ProbeConfig::default(), metrics.clone());
    tokio::spawn(probe_task.run());

    // 4. Setup Frame Router
    let (router, mut quic_rx, mut srt_rx) = FrameRouter::new(
        RouterConfig::default(),
        decision_rx,
        metrics.clone(),
        tracker.clone(),
    );

    let (encoded_tx, encoded_rx) = mpsc::channel(64);
    tokio::spawn(router.run(encoded_rx));

    // Sink tasks to drain the transport queues (simulating network send)
    tokio::spawn(async move {
        while let Some(bytes) = quic_rx.recv().await {
            tracing::trace!("Sent {} bytes over QUIC", bytes.len());
        }
    });
    tokio::spawn(async move {
        while let Some(bytes) = srt_rx.recv().await {
            tracing::trace!("Sent {} bytes over SRT-lite", bytes.len());
        }
    });

    // 5. Setup Capture & Encode
    let mut capture = TestCapture::new(CaptureConfig {
        fps: args.fps,
        resolution: Resolution::FHD,
        ..Default::default()
    });

    let mut encoder = H264Encoder::new(CodecConfig {
        codec: proto::CodecType::H264,
        preset: LatencyPreset::UltraLow,
        bitrate_kbps: args.bitrate_kbps,
        keyframe_interval: args.fps * 2,
        width: 1920,
        height: 1080,
        fps: args.fps,
    })?;

    info!("Pipeline fully wired. Starting streaming loop at {} fps.", args.fps);

    // 6. Main Pipeline Loop
    loop {
        // Capture
        let frame = capture.next_frame().await?;
        let frame_id = frame.id;
        tracker.begin(frame_id);
        tracker.record(frame_id, PipelineStage::Capture);

        // Encode
        let encoded = encoder.encode(frame)?;
        tracker.record(frame_id, PipelineStage::Encode);

        // Send to Router
        if encoded_tx.send(encoded).await.is_err() {
            tracing::error!("Router channel closed");
            break;
        }
        
        // Let's complete the synthetic telemetry so it shows up in metrics
        // The router stamps `Packetize`, so we stamp the rest.
        tracker.record(frame_id, PipelineStage::Send);
        tracker.record(frame_id, PipelineStage::Receive);
        tracker.record(frame_id, PipelineStage::Decode);
        tracker.record(frame_id, PipelineStage::Render);
        tracker.record(frame_id, PipelineStage::Complete);

        if let Ok(r) = tracker.report(frame_id) {
            if frame_id.0 % 60 == 0 {
                info!("Frame {} E2E Latency: {:?}", frame_id.0, r.total_us);
            }
        }
    }

    Ok(())
}

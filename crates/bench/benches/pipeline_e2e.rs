use criterion::{black_box, criterion_group, criterion_main, Criterion};
use proto::{FrameId, PipelineStage, RawFrame};
use std::sync::Arc;
use telemetry::{LatencyTracker, PipelineReport};

/// Simulates a full pipeline pass through 8 stages and measures overhead.
fn simulate_pipeline_pass(tracker: &LatencyTracker) {
    let fid = FrameId::new();
    tracker.begin(fid);
    tracker.record(fid, PipelineStage::Capture);
    tracker.record(fid, PipelineStage::Encode);
    tracker.record(fid, PipelineStage::Packetize);
    tracker.record(fid, PipelineStage::Send);
    tracker.record(fid, PipelineStage::Receive);
    tracker.record(fid, PipelineStage::Decode);
    tracker.record(fid, PipelineStage::Render);
    let _ = tracker.report(fid);
}

fn bench_pipeline_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_e2e");

    group.bench_function("telemetry_8_stage_record", |b| {
        let tracker = LatencyTracker::new();
        b.iter(|| simulate_pipeline_pass(black_box(&tracker)))
    });

    group.bench_function("frame_id_generation", |b| {
        b.iter(|| black_box(FrameId::new()))
    });

    group.bench_function("raw_frame_synthetic_1080p", |b| {
        b.iter(|| black_box(RawFrame::synthetic(1920, 1080, 0)))
    });

    group.finish();
}

criterion_group!(benches, bench_pipeline_e2e);
criterion_main!(benches);

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use simd_codecs::bilinear_scale;

fn bench_bilinear(c: &mut Criterion) {
    let mut group = c.benchmark_group("bilinear_scale");

    // 4K → 1080p (3-channel RGB)
    let src_w = 3840usize;
    let src_h = 2160usize;
    let dst_w = 1920usize;
    let dst_h = 1080usize;
    let channels = 3usize;

    // Use a non-trivial pattern so the compiler can't constant-fold the output.
    let src: Vec<u8> = (0..src_w * src_h * channels)
        .map(|i| (i % 251) as u8)
        .collect();
    let mut dst = vec![0u8; dst_w * dst_h * channels];

    group.bench_function("4K_to_1080p_rgb", |b| {
        b.iter(|| {
            bilinear_scale(
                black_box(&src),
                black_box(&mut dst),
                src_w,
                src_h,
                dst_w,
                dst_h,
                channels,
            )
            .expect("bilinear_scale failed")
        })
    });

    // Also benchmark Y-plane-only (1 channel) as it's the hot path in the encoder.
    let src_y: Vec<u8> = (0..src_w * src_h).map(|i| (16 + i % 220) as u8).collect();
    let mut dst_y = vec![0u8; dst_w * dst_h];

    group.bench_function("4K_to_1080p_luma", |b| {
        b.iter(|| {
            bilinear_scale(
                black_box(&src_y),
                black_box(&mut dst_y),
                src_w,
                src_h,
                dst_w,
                dst_h,
                1,
            )
            .expect("bilinear_scale luma failed")
        })
    });

    group.finish();
}

criterion_group!(benches, bench_bilinear);
criterion_main!(benches);

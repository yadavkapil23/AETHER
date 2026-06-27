use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use simd_codecs::{yuv420_to_rgb24, yuv420_to_rgb24_scalar, CpuFeatures};

/// Generates a realistic YUV420p test frame (Y ramp, neutral U/V)
fn make_yuv_frame(width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let uv_size = (width / 2) * (height / 2);
    let mut data = vec![0u8; y_size + 2 * uv_size];
    for (i, y) in data[..y_size].iter_mut().enumerate() {
        *y = (16 + (i % 220)) as u8;
    }
    for b in &mut data[y_size..] {
        *b = 128;
    }
    data
}

fn bench_codec_comparison(c: &mut Criterion) {
    let features = CpuFeatures::detect();
    println!(
        "\n  CPU Features — AVX2: {}, SSE4.2: {}, NEON: {}",
        features.avx2, features.sse42, features.neon
    );

    let mut group = c.benchmark_group("codec_comparison");
    group.sample_size(15);

    for (label, w, h) in &[
        ("480p",  854usize,  480usize),
        ("720p",  1280usize, 720usize),
        ("1080p", 1920usize, 1080usize),
        ("4K",    3840usize, 2160usize),
    ] {
        let yuv = make_yuv_frame(*w, *h);
        let mut rgb = vec![0u8; w * h * 3];

        group.bench_with_input(
            BenchmarkId::new("yuv420_scalar", label),
            label,
            |b, _| b.iter(|| yuv420_to_rgb24_scalar(black_box(&yuv), black_box(&mut rgb), *w, *h)),
        );

        group.bench_with_input(
            BenchmarkId::new("yuv420_dispatch", label),
            label,
            |b, _| b.iter(|| yuv420_to_rgb24(black_box(&yuv), black_box(&mut rgb), *w, *h)),
        );
    }

    group.finish();
}

criterion_group!(benches, bench_codec_comparison);
criterion_main!(benches);

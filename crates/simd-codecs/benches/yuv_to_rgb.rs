use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use simd_codecs::{yuv420_to_rgb24, yuv420_to_rgb24_scalar, CpuFeatures};

/// Build a realistic YUV420p frame that exercises the coefficient path across
/// a wide value range (Y ramp 16..235, neutral chroma U=V=128).
fn make_yuv_frame(width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let uv_size = (width / 2) * (height / 2);
    let mut data = vec![0u8; y_size + 2 * uv_size];
    // Y plane: ramp 16..235
    for (i, y) in data[..y_size].iter_mut().enumerate() {
        *y = (16 + (i % 220)) as u8;
    }
    // U and V planes: 128 (neutral chroma)
    for b in &mut data[y_size..] {
        *b = 128;
    }
    data
}

fn bench_yuv_to_rgb(c: &mut Criterion) {
    let features = CpuFeatures::detect();
    eprintln!("Benchmarking with {features}");

    let mut group = c.benchmark_group("yuv_to_rgb");
    group.sample_size(20);

    for (label, w, h) in &[
        ("720p", 1280usize, 720usize),
        ("1080p", 1920usize, 1080usize),
        ("4K", 3840usize, 2160usize),
    ] {
        let yuv = make_yuv_frame(*w, *h);
        let mut rgb = vec![0u8; w * h * 3];

        group.bench_with_input(BenchmarkId::new("scalar", label), label, |b, _| {
            b.iter(|| {
                yuv420_to_rgb24_scalar(black_box(&yuv), black_box(&mut rgb), *w, *h)
                    .expect("scalar conversion failed")
            })
        });

        group.bench_with_input(BenchmarkId::new("dispatch", label), label, |b, _| {
            b.iter(|| {
                yuv420_to_rgb24(black_box(&yuv), black_box(&mut rgb), *w, *h)
                    .expect("dispatch conversion failed")
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_yuv_to_rgb);
criterion_main!(benches);

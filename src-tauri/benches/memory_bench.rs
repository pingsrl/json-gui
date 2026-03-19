/// Memory benchmark for JSON file parsing from the samples/ folder.
///
/// For each .json file found in src-tauri/benches/samples/ measures:
///   - RSS delta (process RSS before/after loading)
///   - RSS/file_size ratio (target: < 1.5x, ideal < 1.1x)
///   - Estimated in-memory index size
///
/// Run with:
///   cargo bench --bench memory_bench
///
/// To add a test file just copy it into benches/samples/.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use json_gui_lib::json_index::JsonIndex;
use std::path::Path;
use std::time::Duration;

/// Current process RSS in KB (macOS/Linux).
fn rss_kb() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok();
        out.and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

/// Estimated heap bytes used by the index (excludes allocator overhead).
fn index_heap_bytes(index: &JsonIndex) -> usize {
    index.heap_bytes_estimate()
}

/// Collects all .json files in the samples/ folder relative to src-tauri/.
fn collect_samples() -> Vec<std::path::PathBuf> {
    let samples_dir = Path::new("benches/samples");
    if !samples_dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = std::fs::read_dir(samples_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    files.sort(); // ordine deterministico
    files
}

fn bench_samples(c: &mut Criterion) {
    let samples = collect_samples();

    if samples.is_empty() {
        eprintln!(
            "[memory_bench] No .json files found in benches/samples/\n\
             Copy one or more JSON files there to run the benchmark."
        );
        return;
    }

    for path in &samples {
        let path_str = path.to_str().unwrap_or_default();
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file_mb = file_size as f64 / 1_048_576.0;

        eprintln!(
            "[memory_bench] ── {} ({:.1} MB) ──",
            path.file_name().unwrap_or_default().to_string_lossy(),
            file_mb
        );

        let rss_before = rss_kb();
        let index = match JsonIndex::from_file(path_str) {
            Ok(idx) => idx,
            Err(e) => {
                eprintln!("[memory_bench]   ERROR: {e}");
                continue;
            }
        };
        let rss_after = rss_kb();

        let rss_delta_mb = rss_after.saturating_sub(rss_before) as f64 / 1024.0;
        let index_mb = index_heap_bytes(&index) as f64 / 1_048_576.0;
        let ratio = if file_mb > 0.0 {
            rss_delta_mb / file_mb
        } else {
            0.0
        };

        eprintln!(
            "[memory_bench]   Nodes: {}  |  Index est.: {:.1} MB  |  RSS delta: {:.1} MB  |  Ratio: {:.2}x file",
            index.nodes.len(),
            index_mb,
            rss_delta_mb,
            ratio,
        );

        if rss_delta_mb > 0.0 {
            if ratio < 1.1 {
                eprintln!("[memory_bench]   ✓ Ratio {ratio:.2}x < 1.1x  →  OK");
            } else {
                eprintln!("[memory_bench]   ✗ Ratio {ratio:.2}x >= 1.1x  →  TOO MUCH RAM");
                assert!(
                    ratio < 1.1,
                    "RSS delta {rss_delta_mb:.1} MB is {ratio:.2}x the file ({file_mb:.1} MB): too much RAM used"
                );
            }
        }

        // Benchmark throughput del parsing
        let mut group = c.benchmark_group("memory");
        group.sample_size(10);
        group.warm_up_time(Duration::from_secs(2));
        group.measurement_time(Duration::from_secs(10));
        group.throughput(Throughput::Bytes(file_size));

        let path_clone = path_str.to_string();
        let bench_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        group.bench_function(&bench_name, |b| {
            b.iter(|| {
                JsonIndex::from_file(std::hint::black_box(&path_clone)).expect("parse failed")
            })
        });
        group.finish();
    }
}

criterion_group!(benches, bench_samples);
criterion_main!(benches);

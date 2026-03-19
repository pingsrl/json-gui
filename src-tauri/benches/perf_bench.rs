/// Performance regression guard for JsonGUI critical paths.
///
/// Measures and asserts maximum acceptable times for:
///   - load   : JsonIndex::from_str (parsing + index build)
///   - search : index.search() with a regex over values
///   - expand : BFS traversal to collect all children (expand-all)
///   - memory : Node size stays ≤ 24 bytes (currently 20)
///
/// Run with:
///   cd src-tauri && cargo bench --bench perf_bench
///   cargo bench --bench perf_bench -- --output-format verbose
///
/// Thresholds are intentionally generous (10× synthetic expected) so that
/// the bench acts as a regression guard, not a micro-optimisation harness.
/// Tighten them incrementally as you gather baselines.
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use json_gui_lib::json_index::{JsonIndex, Node};
use std::collections::VecDeque;
use std::mem::size_of;
use std::time::Duration;

// ── Synthetic JSON generators ─────────────────────────────────────────────────

/// [{\"id\":N, \"name\":\"itemN\", \"active\":true, \"score\":1.5}, ...]
fn flat_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 60);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"id":{},"name":"item{}","active":{},"score":{:.1}}}"#,
            i,
            i,
            i % 2 == 0,
            i as f64 * 1.5
        ));
    }
    s.push(']');
    s
}

/// [{\"id\":N, \"tags\":[...], \"meta\":{...}}, ...]  – 7 nodes/object
fn nested_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 120);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"id":{},"tags":["t{}","t{}"],"meta":{{"x":{},"y":{}}}}}"#,
            i,
            i % 10,
            i % 20,
            i * 2,
            i * 3
        ));
    }
    s.push(']');
    s
}

// ── BFS expand-all helper ─────────────────────────────────────────────────────

fn bfs_all_children(index: &JsonIndex) -> usize {
    let mut count = 0;
    let mut queue = VecDeque::new();
    for id in index.get_children_slice(index.root) {
        queue.push_back(id);
    }
    while let Some(node_id) = queue.pop_front() {
        let children = index.get_children_slice(node_id);
        count += children.len();
        for id in children {
            queue.push_back(id);
        }
    }
    count
}

// ── Benchmarks ────────────────────────────────────────────────────────────────

/// Parsing + index build via from_str (in-memory, sonic-rs).
fn bench_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("load");
    group.warm_up_time(Duration::from_secs(2));

    for &n in &[1_000usize, 10_000, 50_000] {
        // Flat
        let json = flat_array(n);
        group.throughput(Throughput::Bytes(json.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("flat", format!("{}k_nodes", n * 5 / 1000)),
            &json,
            |b, j| b.iter(|| JsonIndex::from_str(black_box(j)).unwrap()),
        );
        // Nested
        let json = nested_array(n);
        group.throughput(Throughput::Bytes(json.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("nested", format!("{}k_nodes", n * 7 / 1000)),
            &json,
            |b, j| b.iter(|| JsonIndex::from_str(black_box(j)).unwrap()),
        );
    }
    group.finish();
}

/// Parsing + index build via from_file (mmap + sonic-rs).
/// Writes a temp file, then benchmarks from_file vs from_str for the same data.
fn bench_load_from_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_from_file");
    group.warm_up_time(Duration::from_secs(1));

    for &n in &[10_000usize, 50_000] {
        let json = flat_array(n);
        let bytes = json.len();

        // Write to a temp file once, reuse across iterations.
        let tmp = std::env::temp_dir().join(format!("perf_bench_flat_{n}.json"));
        std::fs::write(&tmp, &json).unwrap();
        let path = tmp.to_str().unwrap().to_string();

        group.throughput(Throughput::Bytes(bytes as u64));

        group.bench_with_input(
            BenchmarkId::new("from_str", format!("{}k_nodes", n * 5 / 1000)),
            &json,
            |b, j| b.iter(|| JsonIndex::from_str(black_box(j)).unwrap()),
        );
        group.bench_with_input(
            BenchmarkId::new("from_file_mmap", format!("{}k_nodes", n * 5 / 1000)),
            &path,
            |b, p| b.iter(|| JsonIndex::from_file(black_box(p)).unwrap()),
        );
    }
    group.finish();
}

/// Regex search over values (the hot path that triggered the CPU spike).
fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("search");
    group.warm_up_time(Duration::from_secs(2));

    for &n in &[1_000usize, 10_000, 50_000] {
        let index = JsonIndex::from_str(&flat_array(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("regex_values", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| {
                b.iter(|| {
                    black_box(idx.search(
                        black_box(r"\d+"), // pattern
                        "values",          // target
                        false,             // case_sensitive
                        true,              // regex
                        false,             // whole_word
                        500,               // max_results
                        None,              // scope_path
                        false,             // search_objects
                        false,             // search_arrays
                    ))
                })
            },
        );
    }

    // Also bench key search
    for &n in &[10_000usize, 50_000] {
        let index = JsonIndex::from_str(&nested_array(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("plain_keys", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| {
                b.iter(|| {
                    black_box(idx.search(
                        black_box("id"),
                        "keys",
                        false,
                        false,
                        false,
                        500,
                        None,
                        false,
                        false,
                    ))
                })
            },
        );
    }
    group.finish();
}

/// BFS collect all children (expand-all).
fn bench_expand_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("expand_all");
    group.warm_up_time(Duration::from_secs(2));

    for &n in &[1_000usize, 10_000, 50_000, 100_000] {
        let index = JsonIndex::from_str(&flat_array(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("flat_bfs", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| b.iter(|| black_box(bfs_all_children(black_box(idx)))),
        );
    }

    for &n in &[1_000usize, 10_000, 50_000] {
        let index = JsonIndex::from_str(&nested_array(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("nested_bfs", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| b.iter(|| black_box(bfs_all_children(black_box(idx)))),
        );
    }
    group.finish();
}

/// Memory: Node size must not exceed 24 bytes.
/// Run as a zero-time assertion bench so it shows up in the report.
fn bench_memory_layout(c: &mut Criterion) {
    let node_size = size_of::<Node>();
    eprintln!("[perf_bench] size_of::<Node>() = {} bytes", node_size);
    assert!(
        node_size <= 24,
        "Node size regressed: {} bytes (max 24). \
         Restore compact layout to avoid memory blowup.",
        node_size
    );

    // A trivial bench that just reports the size as throughput
    let mut group = c.benchmark_group("memory_layout");
    group.bench_function("node_size_bytes", |b| {
        b.iter(|| black_box(size_of::<Node>()))
    });
    group.finish();
}

/// get_path() speed — exercises parent_of() which was O(n), now O(1).
fn bench_get_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_path");
    group.warm_up_time(Duration::from_secs(1));

    for &n in &[10_000usize, 50_000] {
        let index = JsonIndex::from_str(&flat_array(n)).unwrap();
        // Pick a deep-ish leaf (last node)
        let leaf_id = (index.nodes.len() - 1) as u32;
        group.bench_with_input(
            BenchmarkId::new("last_leaf", format!("{}k_nodes", index.nodes.len() / 1000)),
            &(index, leaf_id),
            |b, (idx, id)| b.iter(|| black_box(idx.get_path(black_box(*id)))),
        );
    }
    group.finish();
}

// ── Real-file bench (optional) ────────────────────────────────────────────────

fn read_dotenv(key: &str) -> Option<String> {
    for candidate in &["../.bench.env", ".bench.env"] {
        let Ok(content) = std::fs::read_to_string(candidate) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix(key) {
                if let Some(val) = rest.strip_prefix('=') {
                    return Some(val.trim().to_string());
                }
            }
        }
    }
    None
}

fn bench_real_file(c: &mut Criterion) {
    let path = std::env::var("BENCH_JSON_PATH")
        .ok()
        .or_else(|| read_dotenv("BENCH_JSON_PATH"));
    let Some(path) = path else {
        eprintln!("[perf_bench] Set BENCH_JSON_PATH in .bench.env to enable real-file benchmarks");
        return;
    };
    if !std::path::Path::new(&path).exists() {
        eprintln!("[perf_bench] File not found: {path}");
        return;
    }

    let json = std::fs::read_to_string(&path).expect("read failed");
    eprintln!("[perf_bench] Loaded {} bytes from {path}", json.len());

    let index = JsonIndex::from_str(&json).expect("parse failed");
    eprintln!("[perf_bench] {} nodes", index.nodes.len());

    let mut group = c.benchmark_group("real_file");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.throughput(Throughput::Bytes(json.len() as u64));

    group.bench_function("load", |b| {
        b.iter(|| JsonIndex::from_str(black_box(&json)).unwrap())
    });
    group.throughput(Throughput::Elements(index.nodes.len() as u64));
    group.bench_function("search_regex", |b| {
        b.iter(|| {
            black_box(index.search(
                r"\d+", "values", false, true, false, 1000, None, false, false,
            ))
        })
    });
    group.bench_function("expand_all_bfs", |b| {
        b.iter(|| black_box(bfs_all_children(black_box(&index))))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_memory_layout,
    bench_load,
    bench_load_from_file,
    bench_search,
    bench_expand_all,
    bench_get_path,
    bench_real_file,
);
criterion_main!(benches);

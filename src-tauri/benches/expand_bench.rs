/// Benchmarks for the critical operations of JsonGUI.
///
/// Run with:
///   cd src-tauri && cargo bench
///   cargo bench -- --output-format verbose
///
/// HTML reports in: src-tauri/target/criterion/
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use json_gui_lib::json_index::{JsonIndex, NodeValue};
use std::borrow::Cow;
use std::collections::VecDeque;

// ── Synthetic JSON generators ─────────────────────────────────────────────────

/// Array of N flat objects: [{"id":0,"name":"item0","value":42}, ...]
fn flat_array_json(n: usize) -> String {
    let mut s = String::with_capacity(n * 50);
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
            (i as f64) * 1.5
        ));
    }
    s.push(']');
    s
}

/// Array of N objects with a nested array:
/// [{"id":0,"tags":["a","b","c"],"meta":{"x":1,"y":2}}, ...]
fn nested_array_json(n: usize) -> String {
    let mut s = String::with_capacity(n * 100);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"id":{},"tags":["tag{}","tag{}"],"meta":{{"x":{},"y":{}}}}}"#,
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

// ── Simulation of expand_all logic ───────────────────────────────────────────

/// Reproduces the expand_all BFS without IPC.
/// Measures: time required solely to build the NodeDto objects.
fn bfs_collect_all_dtos(index: &JsonIndex) -> Vec<(u32, Vec<NodeDtoSimple>)> {
    let mut result = Vec::new();
    let mut queue = VecDeque::new();
    for &child_id in index.get_children_slice(index.root) {
        queue.push_back(child_id);
    }
    while let Some(node_id) = queue.pop_front() {
        let children_slice = index.get_children_slice(node_id);
        if children_slice.is_empty() {
            continue;
        }
        let children: Vec<NodeDtoSimple> = children_slice
            .iter()
            .map(|&id| node_to_dto_simple(index, id))
            .collect();
        for &child_id in children_slice {
            queue.push_back(child_id);
        }
        result.push((node_id, children));
    }
    result
}

/// Minimal DTO that mirrors the production structure.
#[derive(Debug)]
struct NodeDtoSimple {
    id: u32,
    key: Option<String>,
    value_type: &'static str,
    value_preview: Cow<'static, str>,
    children_count: usize,
}

fn node_to_dto_simple(index: &JsonIndex, id: u32) -> NodeDtoSimple {
    let node = &index.nodes[id as usize];
    let children_len = node.children_len as usize;
    let (value_type, value_preview): (&'static str, Cow<'static, str>) = match &node.value {
        NodeValue::Object => (
            "object",
            if children_len == 0 { Cow::Borrowed("{}") } else { Cow::Owned(format!("{{{} keys}}", children_len)) },
        ),
        NodeValue::Array => (
            "array",
            if children_len == 0 { Cow::Borrowed("[]") } else { Cow::Owned(format!("[{} items]", children_len)) },
        ),
        NodeValue::Str(s) => (
            "string",
            if s.chars().count() > 80 {
                let end = s.char_indices().nth(80).map(|(i, _)| i).unwrap_or(s.len());
                Cow::Owned(format!("\"{}…\"", &s[..end]))
            } else {
                Cow::Owned(format!("\"{}\"", s))
            },
        ),
        NodeValue::Num(n) => ("number", Cow::Owned(n.to_string())),
        NodeValue::Bool(true) => ("boolean", Cow::Borrowed("true")),
        NodeValue::Bool(false) => ("boolean", Cow::Borrowed("false")),
        NodeValue::Null => ("null", Cow::Borrowed("null")),
    };
    NodeDtoSimple {
        id,
        key: node.key.map(|kid| index.keys.get(kid).to_string()),
        value_type,
        value_preview,
        children_count: children_len,
    }
}

// ── Serde JSON serialization cost ─────────────────────────────────────────────

/// Simulates the cost of serializing get_expanded_slice in the compact tuple format.
/// Each row: [id, parent_id, key_idx, type_byte, preview, children_count, depth]
fn serialize_compact_slice(nodes: &[(u32, &NodeDtoSimple)]) -> String {
    // Local key pool (mirrors the production one)
    let mut key_pool: Vec<&str> = Vec::new();
    let mut key_pool_keys: Vec<Option<&str>> = Vec::new();

    let mut rows = String::with_capacity(nodes.len() * 40);
    rows.push('[');
    for (i, (parent_id, dto)) in nodes.iter().enumerate() {
        if i > 0 { rows.push(','); }
        let key_idx: i32 = match dto.key.as_deref() {
            None => -1,
            Some(k) => {
                match key_pool_keys.iter().position(|&kk| kk == Some(k)) {
                    Some(pos) => pos as i32,
                    None => {
                        let pos = key_pool.len() as i32;
                        key_pool.push(k);
                        key_pool_keys.push(Some(k));
                        pos
                    }
                }
            }
        };
        let type_byte: u8 = match dto.value_type {
            "object" => 0, "array" => 1, "string" => 2,
            "number" => 3, "boolean" => 4, _ => 5,
        };
        let preview = dto.value_preview.replace('"', "\\\"");
        rows.push_str(&format!(
            "[{},{},{},{}",
            dto.id, parent_id, key_idx, type_byte
        ));
        rows.push_str(&format!(",\"{}\",{},0]", preview, dto.children_count));
    }
    rows.push(']');

    // Serialize the key pool
    let mut pool_json = String::from("[");
    for (i, k) in key_pool.iter().enumerate() {
        if i > 0 { pool_json.push(','); }
        pool_json.push('"');
        pool_json.push_str(k);
        pool_json.push('"');
    }
    pool_json.push(']');

    format!(r#"{{"offset":0,"total_count":50000,"key_pool":{},"rows":{}}}"#, pool_json, rows)
}

// ── Criterion groups ──────────────────────────────────────────────────────────

fn bench_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("parsing");
    for &n in &[1_000usize, 10_000, 50_000] {
        let json = flat_array_json(n);
        let bytes = json.len();
        group.throughput(Throughput::Bytes(bytes as u64));
        group.bench_with_input(
            BenchmarkId::new("flat_array", format!("{}k_nodes", n * 5 / 1000)),
            &json,
            |b, j| b.iter(|| JsonIndex::from_str(black_box(j)).unwrap()),
        );
    }
    for &n in &[1_000usize, 10_000, 50_000] {
        let json = nested_array_json(n);
        let bytes = json.len();
        group.throughput(Throughput::Bytes(bytes as u64));
        group.bench_with_input(
            BenchmarkId::new("nested_array", format!("{}k_nodes", n * 7 / 1000)),
            &json,
            |b, j| b.iter(|| JsonIndex::from_str(black_box(j)).unwrap()),
        );
    }
    group.finish();
}

fn bench_bfs_dto(c: &mut Criterion) {
    let mut group = c.benchmark_group("expand_all_bfs");
    for &n in &[1_000usize, 10_000, 50_000, 100_000] {
        let index = JsonIndex::from_str(&flat_array_json(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("flat_dto_build", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| b.iter(|| bfs_collect_all_dtos(black_box(idx))),
        );
    }
    for &n in &[1_000usize, 10_000, 50_000] {
        let index = JsonIndex::from_str(&nested_array_json(n)).unwrap();
        let node_count = index.nodes.len();
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new("nested_dto_build", format!("{}k", node_count / 1000)),
            &index,
            |b, idx| b.iter(|| bfs_collect_all_dtos(black_box(idx))),
        );
    }
    group.finish();
}

fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_serialization");
    // Simulates get_expanded_slice with the compact tuple format.
    // Flattens all (parent_id, Vec<NodeDtoSimple>) pairs into a flat
    // slice matching what get_expanded_slice produces.
    let index = JsonIndex::from_str(&flat_array_json(10_000)).unwrap();
    let all = bfs_collect_all_dtos(&index);
    let flat: Vec<(u32, NodeDtoSimple)> = all.into_iter()
        .flat_map(|(parent_id, children)| children.into_iter().map(move |dto| (parent_id, dto)))
        .collect();
    for &slice_size in &[100usize, 200, 500, 1000] {
        let slice: Vec<(u32, &NodeDtoSimple)> = flat[..slice_size.min(flat.len())]
            .iter().map(|(p, d)| (*p, d)).collect();
        group.throughput(Throughput::Elements(slice.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("compact_slice", slice_size),
            &slice,
            |b, sl| b.iter(|| serialize_compact_slice(black_box(sl))),
        );
    }
    group.finish();
}

fn bench_build_raw(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_raw");
    for &n in &[100usize, 1_000, 10_000] {
        let index = JsonIndex::from_str(&flat_array_json(n)).unwrap();
        group.throughput(Throughput::Elements(index.nodes.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("flat", format!("{}items", n)),
            &index,
            |b, idx: &JsonIndex| b.iter(|| idx.build_raw(black_box(idx.root))),
        );
    }
    group.finish();
}

// ── Benchmark on a real file ──────────────────────────────────────────────────

/// Reads KEY=value from a .env file (only the requested variable, no extra dependencies).
fn read_dotenv(key: &str) -> Option<String> {
    // Look for .bench.env in the workspace root (two levels up from benches/)
    let candidates = ["../.bench.env", ".bench.env"];
    for candidate in candidates {
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

/// Measures BFS traversal on a real JSON file.
/// Looks for BENCH_JSON_PATH in: environment variable, then .bench.env in the project root.
fn bench_real_file(c: &mut Criterion) {
    let path = std::env::var("BENCH_JSON_PATH")
        .ok()
        .or_else(|| read_dotenv("BENCH_JSON_PATH"));
    let Some(path) = path else {
        eprintln!(
            "[bench_real_file] Set BENCH_JSON_PATH in .bench.env or as an environment variable"
        );
        return;
    };

    if !std::path::Path::new(&path).exists() {
        eprintln!("[bench_real_file] File not found: {path}");
        return;
    }

    eprintln!("[bench_real_file] Loading {path} ...");
    let index = JsonIndex::from_file(&path).expect("parse failed");
    let node_count = index.nodes.len();
    eprintln!("[bench_real_file] {node_count} nodes loaded");

    let mut group = c.benchmark_group("real_file");
    // Very large file: a few iterations are enough
    group.sample_size(10);
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.throughput(Throughput::Elements(node_count as u64));

    // 1. BFS only (node count) — isolates traversal overhead
    group.bench_function("bfs_traverse_only", |b| {
        b.iter(|| {
            let mut count = 0u64;
            let mut queue = VecDeque::new();
            for &child_id in index.get_children_slice(index.root) {
                queue.push_back(child_id);
            }
            while let Some(node_id) = queue.pop_front() {
                let children_slice = index.get_children_slice(node_id);
                if children_slice.is_empty() {
                    continue;
                }
                for &child_id in children_slice {
                    queue.push_back(child_id);
                }
                count += children_slice.len() as u64;
            }
            black_box(count)
        });
    });

    // 2. BFS + NodeDto build — simulates the real work of expand_all
    group.bench_function("bfs_dto_build", |b| {
        b.iter(|| bfs_collect_all_dtos(black_box(&index)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parsing,
    bench_bfs_dto,
    bench_serialization,
    bench_build_raw,
    bench_real_file
);
criterion_main!(benches);

/// Benchmark delle operazioni critiche di JsonGUI.
///
/// Esegui con:
///   cd src-tauri && cargo bench
///   cargo bench -- --output-format verbose
///
/// Report HTML in: src-tauri/target/criterion/
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use json_gui_lib::json_index::{JsonIndex, NodeValue};
use std::borrow::Cow;
use std::collections::VecDeque;

// ── Generatori di JSON sintetico ──────────────────────────────────────────────

/// Array di N oggetti piatti: [{"id":0,"name":"item0","value":42}, ...]
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

/// Array di N oggetti con array annidato:
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

// ── Simulazione della logica di expand_all ────────────────────────────────────

/// Riproduce il BFS di expand_all in comandi::expand_all senza IPC.
/// Misura: quanto tempo richiede la sola costruzione dei NodeDto.
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

/// DTO minimalista che replica esattamente la struttura di produzione.
#[derive(Debug)]
struct NodeDtoSimple {
    id: u32,
    key: Option<String>,
    value_type: &'static str,
    value_preview: Cow<'static, str>,
    has_children: bool,
    children_count: usize,
}

fn node_to_dto_simple(index: &JsonIndex, id: u32) -> NodeDtoSimple {
    let node = &index.nodes[id as usize];
    let children_len = node.children_len as usize;
    let has_children = children_len > 0;
    let (value_type, value_preview): (&'static str, Cow<'static, str>) = match &node.value {
        NodeValue::Object => (
            "object",
            if !has_children {
                Cow::Borrowed("{}")
            } else {
                Cow::Owned(format!("{{{} keys}}", children_len))
            },
        ),
        NodeValue::Array => (
            "array",
            if !has_children {
                Cow::Borrowed("[]")
            } else {
                Cow::Owned(format!("[{} items]", children_len))
            },
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
        has_children,
        children_count: children_len,
    }
}

// ── Serde JSON serialization cost ────────────────────────────────────────────

/// Simula il costo di serializzare JSON per l'IPC (serde_json::to_string).
fn serialize_chunk(chunk: &[(u32, Vec<NodeDtoSimple>)]) -> String {
    // Serializzazione manuale per simulare serde
    let mut out = String::with_capacity(chunk.len() * 150);
    out.push('[');
    for (i, (parent_id, children)) in chunk.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("[{},", parent_id));
        out.push('[');
        for (j, dto) in children.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"id":{},"key":{},"value_type":"{}","value_preview":"{}","has_children":{},"children_count":{}}}"#,
                dto.id,
                dto.key.as_deref().map(|k| format!("\"{}\"", k)).unwrap_or_else(|| "null".to_string()),
                dto.value_type,
                dto.value_preview.replace('"', "\\\""),
                dto.has_children,
                dto.children_count
            ));
        }
        out.push_str("]]");
    }
    out.push(']');
    out
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
    // Simula un singolo chunk di 1000 nodi
    let index = JsonIndex::from_str(&flat_array_json(10_000)).unwrap();
    let all = bfs_collect_all_dtos(&index);
    for &chunk_size in &[100usize, 500, 1000, 5000] {
        let chunk = &all[..chunk_size.min(all.len())];
        group.throughput(Throughput::Elements(
            chunk.iter().map(|(_, c)| c.len()).sum::<usize>() as u64,
        ));
        group.bench_with_input(
            BenchmarkId::new("serialize_chunk", chunk_size),
            chunk,
            |b, ch| b.iter(|| serialize_chunk(black_box(ch))),
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

// ── Benchmark su file reale ───────────────────────────────────────────────────

/// Misura il BFS su un file JSON reale.
/// Usa BENCH_JSON_PATH o il path di default se presente.
fn bench_real_file(c: &mut Criterion) {
    let path = std::env::var("BENCH_JSON_PATH").unwrap_or_else(|_| {
        "/Users/g3z/Sviluppo/Mobile/baxter-catalogo/htdocs/dev/data/configurations.json"
            .to_string()
    });

    if !std::path::Path::new(&path).exists() {
        eprintln!("[bench_real_file] File non trovato: {path} — skip");
        return;
    }

    eprintln!("[bench_real_file] Caricamento {path} ...");
    let index = JsonIndex::from_file(&path).expect("parse failed");
    let node_count = index.nodes.len();
    eprintln!("[bench_real_file] {node_count} nodi caricati");

    let mut group = c.benchmark_group("real_file");
    // File molto grande: bastano poche iterazioni
    group.sample_size(10);
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.throughput(Throughput::Elements(node_count as u64));

    // 1. Solo BFS (conteggio nodi) — isola overhead di attraversamento
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

    // 2. BFS + costruzione NodeDto — simula il lavoro reale di expand_all
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

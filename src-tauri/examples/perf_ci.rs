use json_gui_lib::json_index::{
    HeapBytesBreakdown, JsonIndex, Node, ObjectSearchFilter, ObjectSearchOperator,
};
use serde::Serialize;
use std::collections::VecDeque;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_SIZE_MIB: usize = 64;
const DEFAULT_ITERATIONS: usize = 3;
const DEFAULT_MAX_RESULTS: usize = 1_000;
const SEARCH_REGEX_QUERY: &str = r"\d+";
const SEARCH_TEXT_QUERY: &str = "product";
const OBJECT_SEARCH_PATH: &str = "content.mainImage.0.url";
const OBJECT_SEARCH_VALUE: &str = "cdn.example.com/images/";
const LAZY_CATALOG_PATH: &str = "$.catalog";

#[derive(Debug)]
struct Config {
    input: Option<PathBuf>,
    sample_path: PathBuf,
    output: Option<PathBuf>,
    size_mib: usize,
    iterations: usize,
}

#[derive(Serialize)]
struct PerfMetric {
    best_ms: f64,
    median_ms: f64,
    samples_ms: Vec<f64>,
}

#[derive(Serialize)]
struct DatasetInfo {
    path: String,
    generated: bool,
    size_bytes: u64,
    size_mib: f64,
    items: usize,
    node_count: usize,
}

#[derive(Serialize)]
struct SearchInfo {
    query: &'static str,
    mode: &'static str,
    target: &'static str,
    max_results: usize,
    matches: usize,
}

#[derive(Serialize)]
struct ExpandInfo {
    total_descendants: usize,
}

#[derive(Serialize)]
struct ObjectSearchInfo {
    path: &'static str,
    operator: &'static str,
    value: &'static str,
    max_results: usize,
    matches: usize,
}

#[derive(Serialize)]
struct MemoryInfo {
    index_heap_bytes_estimate: usize,
    index_heap_mib_estimate: f64,
    bytes_per_node: f64,
    bytes_per_input_byte: f64,
    node_size_bytes: usize,
    nodes_bytes_estimate: usize,
    parent_index_bytes_estimate: usize,
    container_meta_bytes_estimate: usize,
    keys_bytes_estimate: usize,
    value_strings_bytes_estimate: usize,
    numbers_bytes_estimate: usize,
}

#[derive(Serialize)]
struct PerfReport {
    dataset: DatasetInfo,
    iterations: usize,
    load: PerfMetric,
    search_regex: PerfMetric,
    search_regex_info: SearchInfo,
    search_text: PerfMetric,
    search_text_info: SearchInfo,
    search_objects: PerfMetric,
    search_objects_info: ObjectSearchInfo,
    expand_all: PerfMetric,
    expand: ExpandInfo,
    memory: MemoryInfo,
}

fn print_help() {
    eprintln!(
        "\
Usage: cargo run --release --example perf_ci -- [options]

Options:
  --input PATH         Use an existing JSON file instead of generating one
  --sample-path PATH   Where to write the generated sample
  --size-mib N         Generated sample size in MiB (default: {DEFAULT_SIZE_MIB})
  --iterations N       Measured iterations per operation (default: {DEFAULT_ITERATIONS})
  --output PATH        Write the JSON report to PATH
  -h, --help           Show this help
"
    );
}

fn parse_usize_arg(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("Invalid value for {flag}: {value}"))
}

fn parse_args() -> Result<Config, String> {
    let mut input = None;
    let mut sample_path = None;
    let mut output = None;
    let mut size_mib = DEFAULT_SIZE_MIB;
    let mut iterations = DEFAULT_ITERATIONS;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--input requires a path".to_string())?;
                input = Some(PathBuf::from(value));
            }
            "--sample-path" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--sample-path requires a path".to_string())?;
                sample_path = Some(PathBuf::from(value));
            }
            "--size-mib" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--size-mib requires a number".to_string())?;
                size_mib = parse_usize_arg("--size-mib", &value)?;
            }
            "--iterations" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--iterations requires a number".to_string())?;
                iterations = parse_usize_arg("--iterations", &value)?;
            }
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--output requires a path".to_string())?;
                output = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(format!("Unknown argument: {other}"));
            }
        }
    }

    if iterations == 0 {
        return Err("--iterations must be greater than 0".to_string());
    }

    let sample_path =
        sample_path.unwrap_or_else(|| PathBuf::from("target").join("perf-ci-sample.json"));

    Ok(Config {
        input,
        sample_path,
        output,
        size_mib,
        iterations,
    })
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all({}): {e}", parent.display()))
    } else {
        Ok(())
    }
}

fn generate_sample(path: &Path, size_mib: usize) -> Result<(u64, usize), String> {
    const PREFIX: &str =
        r#"{"catalog":{"name":"ci-perf","generatedAt":"2026-03-19T00:00:00Z","items":["#;
    const SUFFIX: &str = r#"]}}"#;
    let target_bytes = size_mib * 1024 * 1024;

    ensure_parent_dir(path)?;
    let mut file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    file.write_all(PREFIX.as_bytes())
        .map_err(|e| format!("write prefix {}: {e}", path.display()))?;

    let mut approx_written = PREFIX.len() + SUFFIX.len();
    let mut item = String::with_capacity(512);
    let mut items = 0usize;

    loop {
        item.clear();
        let i = items;
        let active = if i % 2 == 0 { "true" } else { "false" };
        let price = i as f64 * 1.37 + 19.99;
        let weight = i as f64 * 0.01 + 0.75;
        write!(
            &mut item,
            concat!(
                "{{",
                "\"id\":{i},",
                "\"sku\":\"SKU-{i:08}\",",
                "\"active\":{active},",
                "\"price\":{price:.2},",
                "\"stock\":{stock},",
                "\"tags\":[\"group-{group}\",\"finish-{finish}\",\"region-{region}\"],",
                "\"metrics\":{{\"width\":{width},\"height\":{height},\"depth\":{depth},\"weight\":{weight:.2}}},",
                "\"content\":{{",
                "\"title\":\"Product {i}\",",
                "\"mainImage\":[{{",
                "\"url\":\"https://cdn.example.com/images/{i:08}.jpg\",",
                "\"width\":1200,",
                "\"height\":900",
                "}}]",
                "}}",
                "}}"
            ),
            i = i,
            active = active,
            price = price,
            weight = weight,
            stock = (i % 5_000) + 1,
            group = i % 32,
            finish = i % 8,
            region = i % 5,
            width = 10 + (i % 200),
            height = 20 + (i % 150),
            depth = 5 + (i % 75),
        )
        .expect("formatting sample item");

        let separator_len = usize::from(items > 0);
        let next_size = approx_written + separator_len + item.len();
        if items > 0 && next_size > target_bytes {
            break;
        }

        if items > 0 {
            file.write_all(b",")
                .map_err(|e| format!("write separator {}: {e}", path.display()))?;
        }
        file.write_all(item.as_bytes())
            .map_err(|e| format!("write item {}: {e}", path.display()))?;
        approx_written = next_size;
        items += 1;
    }

    file.write_all(SUFFIX.as_bytes())
        .map_err(|e| format!("write suffix {}: {e}", path.display()))?;
    file.flush()
        .map_err(|e| format!("flush {}: {e}", path.display()))?;

    let size_bytes = fs::metadata(path)
        .map_err(|e| format!("metadata {}: {e}", path.display()))?
        .len();
    Ok((size_bytes, items))
}

fn timed<T, F>(iterations: usize, mut f: F) -> Result<(PerfMetric, T), String>
where
    F: FnMut() -> Result<T, String>,
{
    let _ = f()?;
    let mut samples = Vec::with_capacity(iterations);
    let mut last = None;

    for _ in 0..iterations {
        let start = Instant::now();
        let value = f()?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        samples.push(elapsed_ms);
        last = Some(value);
    }

    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    let median_ms = if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    };
    let best_ms = sorted[0];

    Ok((
        PerfMetric {
            best_ms,
            median_ms,
            samples_ms: samples,
        },
        last.expect("timed() requires at least one iteration"),
    ))
}

fn bfs_expand_all(index: &JsonIndex) -> usize {
    let mut count = 0usize;
    let mut queue = VecDeque::new();
    for id in index.get_children_any(index.root).unwrap_or_default() {
        queue.push_back(id);
    }
    while let Some(node_id) = queue.pop_front() {
        let children = index.get_children_any(node_id).unwrap_or_default();
        count += children.len();
        for id in children {
            queue.push_back(id);
        }
    }
    count
}

fn lazy_catalog_node_id(index: &JsonIndex) -> Option<u32> {
    let node_id = index.resolve_path_any(LAZY_CATALOG_PATH)?;
    let node = &index.nodes[node_id as usize];
    if node.kind().is_lazy() {
        Some(node_id)
    } else {
        None
    }
}

fn main() -> Result<(), String> {
    let config = parse_args()?;
    let (path, generated, size_bytes, items) = if let Some(input) = config.input.as_ref() {
        let size_bytes = fs::metadata(input)
            .map_err(|e| format!("metadata {}: {e}", input.display()))?
            .len();
        (input.clone(), false, size_bytes, 0usize)
    } else {
        let (size_bytes, items) = generate_sample(&config.sample_path, config.size_mib)?;
        (config.sample_path.clone(), true, size_bytes, items)
    };

    let path_str = path
        .to_str()
        .ok_or_else(|| format!("Non UTF-8 path: {}", path.display()))?;

    let (load_metric, index) = timed(config.iterations, || JsonIndex::from_file(path_str))?;
    let node_count = index.nodes.len();

    let lazy_items_id = lazy_catalog_node_id(&index);
    let (search_regex_metric, search_regex_matches) = timed(config.iterations, || {
        if let Some(node_id) = lazy_items_id {
            index.search_in_lazy_node_with_options(
                node_id,
                SEARCH_REGEX_QUERY,
                "values",
                false,
                true,
                false,
                DEFAULT_MAX_RESULTS,
                false,
                false,
            )
        } else {
            Ok(index.search(
                SEARCH_REGEX_QUERY,
                "values",
                false,
                true,
                false,
                DEFAULT_MAX_RESULTS,
                None,
                false,
                false,
            ))
        }
    })?;
    let (search_text_metric, search_text_matches) = timed(config.iterations, || {
        if let Some(node_id) = lazy_items_id {
            index.search_in_lazy_node_with_options(
                node_id,
                SEARCH_TEXT_QUERY,
                "values",
                false,
                false,
                false,
                DEFAULT_MAX_RESULTS,
                false,
                false,
            )
        } else {
            Ok(index.search(
                SEARCH_TEXT_QUERY,
                "values",
                false,
                false,
                false,
                DEFAULT_MAX_RESULTS,
                None,
                false,
                false,
            ))
        }
    })?;
    let object_filters = [ObjectSearchFilter {
        path: OBJECT_SEARCH_PATH.to_string(),
        operator: ObjectSearchOperator::Contains,
        value: Some(OBJECT_SEARCH_VALUE.to_string()),
        ..Default::default()
    }];
    let (search_objects_metric, search_objects_matches) = timed(config.iterations, || {
        if let Some(node_id) = lazy_items_id {
            index.search_objects_in_lazy_node(
                node_id,
                &object_filters,
                true,
                false,
                DEFAULT_MAX_RESULTS,
            )
        } else {
            Ok(index.search_objects(&object_filters, true, false, DEFAULT_MAX_RESULTS, None))
        }
    })?;

    let (expand_metric, descendants) = timed(config.iterations, || Ok(bfs_expand_all(&index)))?;
    let HeapBytesBreakdown {
        nodes: nodes_bytes_estimate,
        parent_index: parent_index_bytes_estimate,
        container_meta: container_meta_bytes_estimate,
        keys: keys_bytes_estimate,
        val_strings: value_strings_bytes_estimate,
        nums_pool: numbers_bytes_estimate,
    } = index.heap_bytes_breakdown();
    let index_heap_bytes_estimate = index.heap_bytes_estimate();
    let bytes_per_node = if node_count == 0 {
        0.0
    } else {
        index_heap_bytes_estimate as f64 / node_count as f64
    };
    let bytes_per_input_byte = if size_bytes == 0 {
        0.0
    } else {
        index_heap_bytes_estimate as f64 / size_bytes as f64
    };

    let report = PerfReport {
        dataset: DatasetInfo {
            path: path.display().to_string(),
            generated,
            size_bytes,
            size_mib: size_bytes as f64 / (1024.0 * 1024.0),
            items,
            node_count,
        },
        iterations: config.iterations,
        load: load_metric,
        search_regex: search_regex_metric,
        search_regex_info: SearchInfo {
            query: SEARCH_REGEX_QUERY,
            mode: "regex",
            target: "values",
            max_results: DEFAULT_MAX_RESULTS,
            matches: search_regex_matches.len(),
        },
        search_text: search_text_metric,
        search_text_info: SearchInfo {
            query: SEARCH_TEXT_QUERY,
            mode: "text",
            target: "values",
            max_results: DEFAULT_MAX_RESULTS,
            matches: search_text_matches.len(),
        },
        search_objects: search_objects_metric,
        search_objects_info: ObjectSearchInfo {
            path: OBJECT_SEARCH_PATH,
            operator: "contains",
            value: OBJECT_SEARCH_VALUE,
            max_results: DEFAULT_MAX_RESULTS,
            matches: search_objects_matches.len(),
        },
        expand_all: expand_metric,
        expand: ExpandInfo {
            total_descendants: descendants,
        },
        memory: MemoryInfo {
            index_heap_bytes_estimate,
            index_heap_mib_estimate: index_heap_bytes_estimate as f64 / (1024.0 * 1024.0),
            bytes_per_node,
            bytes_per_input_byte,
            node_size_bytes: std::mem::size_of::<Node>(),
            nodes_bytes_estimate,
            parent_index_bytes_estimate,
            container_meta_bytes_estimate,
            keys_bytes_estimate,
            value_strings_bytes_estimate,
            numbers_bytes_estimate,
        },
    };

    if let Some(output) = config.output.as_ref() {
        ensure_parent_dir(output)?;
        let bytes =
            serde_json::to_vec_pretty(&report).map_err(|e| format!("serialize report: {e}"))?;
        fs::write(output, bytes).map_err(|e| format!("write {}: {e}", output.display()))?;
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| format!("serialize report: {e}"))?
        );
    }

    Ok(())
}

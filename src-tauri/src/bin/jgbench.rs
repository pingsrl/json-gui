//! CLI benchmark tool for JsonGUI.
//!
//! Build (release):
//!   cd src-tauri && cargo build --release --bin jgbench
//!
//! Run:
//!   ./target/release/jgbench /path/to/file.json [--search TEXT] [--expand N]
//!
//! Misura per ogni fase:
//!   • Tempo elapsed
//!   • RSS (resident set size) del processo
//!   • Conteggio nodi / risultati

use json_gui_lib::json_index::JsonIndex;
use std::env;
use std::time::Instant;

// ── RSS helpers ───────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn rss_mb() -> f64 {
    // Use ps for simplicity and portability
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok();
    out.and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rss_mb() -> f64 {
    0.0
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn fmt_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.2} s", d.as_secs_f64())
    }
}

fn sep() {
    println!("{}", "─".repeat(60));
}

fn phase(label: &str, elapsed: std::time::Duration, rss_before: f64, rss_after: f64, extra: &str) {
    println!(
        "  {:<22} {:>10}   RSS {:.0}→{:.0} MB ({:+.0} MB)  {}",
        label,
        fmt_duration(elapsed),
        rss_before,
        rss_after,
        rss_after - rss_before,
        extra,
    );
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!(
            "Usage: jgbench <file.json> [--search TEXT] [--expand-rows N]\n\
             \n\
             Misura tempo e RAM per:\n\
               1. Caricamento file (from_file)\n\
               2. Nodi radice (get_children root)\n\
               3. Figli del primo container (get_children)\n\
               4. Ricerca testo (--search, opzionale)\n\
               5. Espansione N righe di array (--expand-rows, default 5)\n\
             \n\
             Esempio:\n\
               jgbench /Downloads/rows.json --search \"THEFT\" --expand-rows 10\n"
        );
        std::process::exit(1);
    }

    let file_path = &args[1];

    // Parse optional flags
    let search_text: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--search")
        .map(|w| w[1].clone());

    let expand_rows: usize = args
        .windows(2)
        .find(|w| w[0] == "--expand-rows")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(5);

    let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
    let file_mb = file_size as f64 / 1_048_576.0;

    println!();
    sep();
    println!("  jgbench — {}", file_path);
    println!("  File size: {:.1} MB", file_mb);
    sep();

    // ── 1. Load ───────────────────────────────────────────────────────────────
    let rss0 = rss_mb();
    let t0 = Instant::now();
    let index = match JsonIndex::from_file(file_path) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("  ERROR loading file: {e}");
            std::process::exit(2);
        }
    };
    let load_elapsed = t0.elapsed();
    let rss1 = rss_mb();

    let node_count = index.nodes.len();
    let lazy_count = index.nodes.iter().filter(|n| n.kind().is_lazy()).count();
    let heap_mb = index.heap_bytes_estimate() as f64 / 1_048_576.0;
    let ram_ratio = if file_mb > 0.0 {
        (rss1 - rss0) / file_mb
    } else {
        0.0
    };

    phase(
        "1. Caricamento",
        load_elapsed,
        rss0,
        rss1,
        &format!("nodes={node_count} lazy={lazy_count} heap={heap_mb:.0}MB ratio={ram_ratio:.2}x"),
    );

    // ── 2. Root children ──────────────────────────────────────────────────────
    let rss_before = rss_mb();
    let t = Instant::now();
    let root_children: Vec<u32> = index.children_iter(index.root).collect();
    let elapsed = t.elapsed();
    let rss_after = rss_mb();
    phase(
        "2. Root children",
        elapsed,
        rss_before,
        rss_after,
        &format!("count={}", root_children.len()),
    );

    // ── 3. First large container ──────────────────────────────────────────────
    // Find the first child with the most children (likely the data array)
    let big_child = root_children
        .iter()
        .copied()
        .max_by_key(|&id| index.children_count_any(id));

    if let Some(big_id) = big_child {
        let big_count = index.children_count_any(big_id);
        let rss_before = rss_mb();
        let t = Instant::now();
        let big_children: Vec<u32> = index.get_children_any(big_id).unwrap_or_default();
        let elapsed = t.elapsed();
        let rss_after = rss_mb();
        phase(
            "3. Primo big container",
            elapsed,
            rss_before,
            rss_after,
            &format!(
                "node={big_id} declared={big_count} got={}",
                big_children.len()
            ),
        );

        // ── 4. Expand N rows ──────────────────────────────────────────────────
        let n = expand_rows.min(big_children.len());
        if n > 0 {
            let rss_before = rss_mb();
            let t = Instant::now();
            let mut total_cells = 0usize;
            for &row_id in big_children.iter().take(n) {
                let cells = index.get_children_any(row_id).unwrap_or_default();
                total_cells += cells.len();
            }
            let elapsed = t.elapsed();
            let rss_after = rss_mb();
            phase(
                &format!("4. Espandi {n} righe"),
                elapsed,
                rss_before,
                rss_after,
                &format!(
                    "celle_tot={total_cells} ~{:.1}ms/riga",
                    elapsed.as_secs_f64() * 1000.0 / n as f64
                ),
            );
        }
    }

    // ── 5. Search ─────────────────────────────────────────────────────────────
    if let Some(ref text) = search_text {
        let rss_before = rss_mb();
        let t = Instant::now();
        let mut results = index.search(text, "both", false, false, false, 500, None, false, false);
        if results.len() < 500 {
            for (id, node) in index.nodes.iter().enumerate() {
                if !node.kind().is_lazy() {
                    continue;
                }
                let remaining = 500usize.saturating_sub(results.len());
                if remaining == 0 {
                    break;
                }
                if let Ok(lazy_results) = index.search_in_lazy_node_with_options(
                    id as u32, text, "both", false, false, false, remaining, false, false,
                ) {
                    results.extend(lazy_results);
                }
                if results.len() >= 500 {
                    break;
                }
            }
        }
        let elapsed = t.elapsed();
        let rss_after = rss_mb();
        phase(
            &format!("5. Ricerca \"{text}\""),
            elapsed,
            rss_before,
            rss_after,
            &format!("risultati={}", results.len()),
        );
    }

    // ── 6. get_path sample ────────────────────────────────────────────────────
    if node_count > 0 {
        let sample_id = (node_count / 2) as u32;
        let rss_before = rss_mb();
        let t = Instant::now();
        let path = index.get_path(sample_id);
        let elapsed = t.elapsed();
        let rss_after = rss_mb();
        phase(
            "6. get_path(mid)",
            elapsed,
            rss_before,
            rss_after,
            &format!("id={sample_id} path=\"{}\"", &path[..path.len().min(40)]),
        );
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    sep();
    println!(
        "  Totale tempo caricamento : {}",
        fmt_duration(load_elapsed)
    );
    println!("  Nodi principali          : {node_count} (lazy={lazy_count})");
    println!("  Heap stimato             : {heap_mb:.1} MB");
    println!(
        "  RSS delta caricamento    : {:.0} MB  ({ram_ratio:.2}x file)",
        rss1 - rss0
    );
    sep();
    println!();
}

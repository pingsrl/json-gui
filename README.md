![CI](https://github.com/pingsrl/json-gui/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)

# JsonGUI

A fast, native desktop app for exploring and searching large JSON files.
Built with Tauri 2 + React 19 + TypeScript.

<img src="docs/screenshot.jpg" alt="JsonGUI screenshot" width="800" />

---

## Features

- Open and explore JSON files of any size (lazy tree loading)
- Full-text search across keys, values, or both — powered by Rayon for parallel execution
- Right-click context menu: copy JSONPath, value, or raw JSON subtree
- Keyboard navigation in the tree (Arrow keys, Enter)
- Recent files list (last 5, persisted in localStorage)
- Drag and drop a JSON file from Finder/Explorer directly onto the window
- Status bar with node count, file size, and path
- Dark UI with Tailwind CSS

---

## Requirements

- macOS 12+ (Apple Silicon or Intel) — primary target
- [Rust stable toolchain](https://rustup.rs/)
- Node.js 22+
- npm 10+

---

## Install

```bash
npm install
```

---

## Usage

Launch in development mode (hot-reload):

```bash
npm run tauri:dev
```

Build a release bundle:

```bash
npm run tauri build
```

On macOS, `npm run tauri build` now creates the `.app` with Tauri and then
generates the `.dmg` from an external staging directory to avoid the `create-dmg`
temporary-image recursion issue.

---

## Development

```bash
# TypeScript type check only
npx tsc --noEmit

# Rust check only
cd src-tauri && cargo check

# Rust tests
cd src-tauri && cargo test
```

---

## Benchmarks

### Rust (Criterion)

Measures parsing, BFS + DTO build, IPC serialization and `build_raw` on synthetic data:

```bash
cd src-tauri && cargo bench
# HTML reports: src-tauri/target/criterion/
```

To benchmark against a real JSON file, create `.bench.env` in the project root
(already in `.gitignore`) with the path to your large file:

```bash
cp .bench.env.example .bench.env
# edit .bench.env:
# BENCH_JSON_PATH=/path/to/large.json

cd src-tauri && cargo bench -- real_file
```

### JavaScript (frontend)

Measures `buildVisibleNodes`, Map copy cost, and streaming overhead on synthetic trees:

```bash
npx tsx src/bench/perf.mts
```

### Reference results (Apple M-series, synthetic data)

| Operation | Size | Time |
|---|---|---|
| Parsing (flat array) | 50k nodes / ~2 MB | 4 ms |
| Parsing (flat array) | 250k nodes / ~10 MB | ~20 ms |
| BFS + DTO build | 50k nodes | 2.4 ms |
| BFS + DTO build | 250k nodes | 12 ms |
| BFS + DTO build | 400k nodes | 21 ms |
| Chunk serialization | 1 000 nodes | 0.6 ms |
| buildVisibleNodes (JS) | 100k nodes | ~2.5 ms |
| buildVisibleNodes (JS) | 500k nodes | ~20 ms |
| Map copy (JS) | 100k entries | ~4 ms |

**IPC note**: the bottleneck for `expand_all` on large files is the number of
`app.emit()` calls, not the Rust BFS itself. `EXPAND_CHUNK_SIZE = 10_000` keeps
call count to ~100 for a 1M-node file (vs 1 000 with chunk size 1k), reducing
IPC overhead by 10×.

---

## Architecture

```
Frontend (React + Zustand)
  TreeNode (lazy expand)   SearchPanel
          |                     |
          +------Tauri IPC------+
                     |
Backend (Rust)
  JsonIndex (arena Vec<Node>)
  SearchEngine (rayon parallel)
  FileLoader (sonic-rs SIMD parser)
```

### Key technical choices

**sonic-rs** — Replaces `serde_json` for the initial JSON parsing phase.
Uses SIMD instructions (NEON on Apple Silicon, AVX2 on x86_64) for significantly
faster deserialization of large files. Produces a standard `serde_json::Value`
so the rest of the tree-building code is unchanged.

**Arena allocation** — The entire JSON tree is stored as a flat `Vec<Node>`
where each node holds integer indices to its children. This avoids recursive
heap allocations and keeps the data cache-friendly. Navigation is O(1) by index.

**Lazy loading** — The frontend only requests children for nodes the user expands
via the `get_children` IPC command. Root children are returned with `open_file`.
This keeps initial load fast regardless of file size.

**Rayon** — The `search` function uses `par_iter()` to scan all nodes in parallel,
distributing work across all CPU cores automatically.

---

## License

MIT

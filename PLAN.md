# JsonGUI — Piano di sviluppo

## Obiettivo

App desktop Tauri per aprire, esplorare e cercare all'interno di file JSON di grandi dimensioni con la massima efficienza. Interfaccia minimalista focalizzata su velocità e usabilità.

---

## Stack tecnico

### Backend (Rust)
- **Tauri 2.x** — shell nativa, IPC, accesso filesystem
- **sonic-rs** (`cloudwego/sonic-rs`) — parsing JSON SIMD-accelerated, il più veloce disponibile in Rust per x86/ARM
  - Fallback: **simd-json** se sonic-rs non supporta la piattaforma target
  - Per file molto grandi (>100MB): parsing lazy/streaming con **json-event-parser** o **ijson**-style iteration
- **serde_json** — solo per serializzazione di risposte IPC verso il frontend (strutture piccole)
- **rayon** — parallelismo per ricerche su alberi grandi
- **memmap2** — memory-mapped file I/O per file >50MB

### Frontend
- **React 18** + **TypeScript**
- **Vite** come bundler
- **Tailwind CSS** — styling utility-first
- **@tanstack/react-virtual** — virtualizzazione lista/albero (gestisce migliaia di nodi senza lag)
- **zustand** — state management leggero

---

## Architettura

```
┌─────────────────────────────────────┐
│  Frontend (React)                   │
│  ┌─────────┐  ┌──────────────────┐  │
│  │TreeView │  │  SearchPanel     │  │
│  │(virtual)│  │  (query + results│  │
│  └────┬────┘  └────────┬─────────┘  │
│       └───────┬────────┘            │
│          Tauri IPC                  │
└───────────────┼─────────────────────┘
                │
┌───────────────▼─────────────────────┐
│  Backend (Rust)                     │
│  ┌──────────┐  ┌───────────────┐    │
│  │FileLoader│  │  JsonIndex    │    │
│  │sonic-rs  │  │  (path→value) │    │
│  └──────────┘  └───────────────┘    │
│  ┌──────────────────────────────┐   │
│  │  SearchEngine (rayon)        │   │
│  └──────────────────────────────┘   │
└─────────────────────────────────────┘
```

### Modello dati interno

Il JSON viene parsato in una struttura **arena-allocated** piatta (`Vec<Node>`) con indici padre/figlio. Questo evita allocazioni ricorsive e rende la navigazione O(1).

```rust
struct Node {
    id: u32,
    parent: Option<u32>,
    key: Option<StringId>,   // StringId = indice in string_pool
    value: NodeValue,
    children_start: u32,
    children_len: u32,
}
enum NodeValue {
    Object, Array,
    Str(StringId), Num(f64), Bool(bool), Null,
}
```

---

## Comandi Tauri IPC

| Comando | Input | Output |
|---|---|---|
| `open_file` | `path: String` | `FileInfo { node_count, depth, size_bytes }` |
| `get_children` | `node_id: u32, offset: u32, limit: u32` | `Vec<NodeDto>` |
| `get_path` | `node_id: u32` | `String` (JSONPath) |
| `search` | `query: SearchQuery` | `Vec<SearchResult>` |
| `get_raw` | `node_id: u32` | `String` (JSON raw del sottoalbero) |
| `expand_to` | `jsonpath: String` | `Vec<u32>` (ids da espandere) |

### SearchQuery
```typescript
interface SearchQuery {
  text: string;          // testo da cercare
  target: 'keys' | 'values' | 'both';
  case_sensitive: boolean;
  regex: boolean;
  max_results: number;   // default 500
}
```

---

## UI — Layout

```
┌─────────────────────────────────────────────────┐
│ [📂 Apri file]  path/al/file.json  [⟳]  [⚙]   │  ← Toolbar
├────────────────────┬────────────────────────────┤
│                    │ 🔍 [___________________]   │  ← Search bar
│  Tree Explorer     │  ○ chiavi  ○ valori  ○ entrambi │
│                    │  □ regex  □ case sensitive  │
│  ▶ root (object)   ├────────────────────────────┤
│    ▼ users (array) │ Risultati (243)             │
│      ▶ 0 (object)  │  > users[0].name "Alice"   │
│      ▶ 1 (object)  │  > users[1].name "Bob"     │
│    ▶ config        │  ...                        │
│                    │                             │
├────────────────────┴────────────────────────────┤
│ Nodi: 12.847  |  Profondità: 8  |  2.3 MB       │  ← Status bar
└─────────────────────────────────────────────────┘
```

### Comportamenti chiave

- **Click** su nodo → espande/collassa
- **Click destro** → copia path / copia valore / copia sottoalbero JSON
- **Hover** su valore lungo → tooltip con valore completo
- **F** o **Cmd+F** → focus sulla search bar
- **Frecce** → navigazione tastiera nel tree
- Drag & drop file sulla finestra per aprirlo
- I nodi vengono caricati **lazy**: solo i figli visibili sono richiesti al backend

---

## Fasi di sviluppo

### Fase 1 — Core (MVP) ✅ completata 2026-03-15
- [x] Setup Tauri 2 + React + Vite + Tailwind CSS
- [x] Struttura arena `JsonIndex` con `Vec<Node>` e build_tree ricorsivo (`src-tauri/src/json_index.rs`)
- [x] Comandi IPC: `open_file`, `get_children`, `get_path`, `get_raw` (`src-tauri/src/commands.rs`)
- [x] TreeView con espansione lazy (caricamento figli on-demand) (`src/components/TreeNode.tsx`)
- [x] Status bar con metadati file (nodi, dimensione, path)
- [x] zustand store con state management (`src/store.ts`)
- [x] TypeScript senza errori, `cargo check` OK
- [x] sonic-rs — integrato in Fase 4 (2026-03-15); `cargo check` OK

### Fase 2 — Ricerca ✅ completata 2026-03-15
- [x] SearchEngine con rayon (parallelismo) nella funzione `JsonIndex::search`
- [x] Ricerca su chiavi / valori / entrambi con opzione case-sensitive
- [x] Pannello risultati con path JSONPath e preview valore
- [x] Shortcut Cmd+F / Ctrl+F per focus search bar
- [x] Support regex via crate `regex` — implementato in `json_index.rs` con `Regex::new`, checkbox UI in `App.tsx`
- [x] `expand_to` per navigare automaticamente al risultato selezionato — implementato in `commands.rs` + `store.ts`

### Fase 3 — UX ✅ completata 2026-03-15
- [x] Drag & drop apertura file (overlay visivo + Tauri `onDragDropEvent`)
- [x] Click su risultato ricerca → espande albero e scrolla al nodo (`expand_to` IPC + highlight)
- [x] Shortcut Cmd+F / Ctrl+F → focus search bar
- [x] Navigazione da tastiera completa (ArrowUp/Down/Left/Right/Enter sul tree)
- [x] Context menu click destro (copia path / copia valore / copia raw JSON)
- [x] Cronologia file recenti (max 5, persistita in localStorage, dropdown in toolbar)
- [x] Tema chiaro/scuro — `@custom-variant dark` in `index.css`, toggle Sun/Moon in toolbar, persistito in localStorage

### Fase 4 — Performance avanzata ✅ completata 2026-03-15
- [x] Integrazione sonic-rs al posto di serde_json per il parsing iniziale (SIMD NEON/AVX2)
  - Nota: compilato e funzionante su macOS ARM (Apple Silicon) con NEON
  - API compatibile con serde_json: `sonic_rs::from_str` restituisce `serde_json::Value`
- [x] Memory-mapped I/O per file >50MB — `JsonIndex::from_file` con `memmap2` in `json_index.rs`
- [x] Parsing streaming per file >200MB — `JsonIndex::from_reader` con `serde_json::from_reader` (no allocazione stringa intera); `ProgressReader` in `commands.rs` emette eventi Tauri `parse-progress` (0-100%); progress bar in `App.tsx`
- [x] Worker thread dedicato per non bloccare UI — `open_file` usa `tauri::async_runtime::spawn_blocking` in `commands.rs`
- [x] Cache LRU dei nodi espansi — Map FIFO con limite 2000 in `store.ts` (`childrenCache`)

---

## Benchmark obiettivo

| File | Dimensione | Target parse | Target search |
|---|---|---|---|
| Piccolo | <1 MB | <10ms | <5ms |
| Medio | 10 MB | <150ms | <30ms |
| Grande | 100 MB | <2s | <500ms |
| XL | 500 MB | streaming | <3s |

---

## Note su sonic-rs

- Richiede CPU con istruzione AVX2 (x86_64) o NEON (ARM/Apple Silicon) — entrambe presenti sui Mac moderni
- API compatibile con serde_json per la maggior parte dei casi
- Per il parsing lazy (get value by path senza parsare tutto) usare `sonic_rs::get` / `sonic_rs::pointer`
- Valutare **gjson** (port Rust) per query JSONPath dirette su raw bytes se non è necessaria la struttura ad albero completa

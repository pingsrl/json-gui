# JsonGUI — Piano di Sviluppo

## Obiettivo

App desktop cross-platform (Mac, Linux, Windows) per visualizzare e interrogare file JSON
di grandi dimensioni (1 GB+) con interfaccia grafica moderna, senza mai caricare l'intero
file in memoria.

---

## Stack Tecnologico

| Layer | Tecnologia | Motivazione |
|---|---|---|
| Shell desktop | **Tauri 2** (Rust) | Leggero, cross-platform, accesso nativo FS |
| UI Framework | **Svelte 5** | Reattivo, bundle piccolo, ottimo con Tauri |
| Virtual scrolling | **TanStack Virtual** | Render solo righe visibili, RAM costante |
| Editor query | **Monaco Editor** | Syntax highlight per SQL/jq/JSONPath |
| Motore query | **DuckDB** (`duckdb-rs`) | SQL su JSON multi-GB senza caricare in RAM |
| Parser streaming | **sonic-rs** | SIMD, ~4x più veloce di serde_json |
| Query jq-style | **jaq** | Crate Rust jq-compatibile |
| Query JSONPath | **jsonpath-rust** | Sintassi `$.store.book[*]` |
| Styling | **Tailwind CSS 4** | Utility-first, build veloce |

---

## Architettura

```
┌─────────────────────────────────────────────────────┐
│                  Tauri 2 Shell                      │
│                                                     │
│  ┌──────────────────────┐  ┌─────────────────────┐  │
│  │   Frontend (Svelte)  │  │   Backend (Rust)    │  │
│  │                      │  │                     │  │
│  │  • FileOpener        │  │  • FileManager      │  │
│  │  • TreeView          │◄─►  • StreamingParser  │  │
│  │  • QueryEditor       │  │  • DuckDBEngine     │  │
│  │  • ResultTable       │  │  • JaqEngine        │  │
│  │  • StatusBar         │  │  • JsonPathEngine   │  │
│  │  • TanStack Virtual  │  │  • PaginatedResults │  │
│  └──────────────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

### Flusso dati

```
File JSON (1GB+)
     │
     ▼
StreamingParser (sonic-rs)          ← scan top-level keys, struttura
     │
     ├──► TreeView (struttura JSON navigabile)
     │
     ▼
DuckDB / jaq / jsonpath-rust        ← query engine selezionato dall'utente
     │
     ▼
PaginatedResults (chunk da 100-500 righe)
     │
     ▼
TanStack Virtual (render solo righe visibili)
```

---

## Funzionalità

### MVP (v0.1)

- [ ] Apertura file JSON tramite dialog nativo
- [ ] Tree view della struttura (lazy, non carica tutto)
- [ ] Visualizzazione raw con virtual scrolling
- [ ] Query SQL via DuckDB con risultati paginati
- [ ] Esportazione risultati in JSON / CSV
- [ ] Status bar con info file (dimensione, path, record count)

### v0.2

- [ ] Query jq-style via jaq
- [ ] Query JSONPath via jsonpath-rust
- [ ] Tab multipli per file diversi
- [ ] Storico query
- [ ] Syntax highlight errori query in tempo reale

### v0.3

- [ ] Supporto JSONL (JSON Lines / NDJSON)
- [ ] Supporto JSON compresso (gzip)
- [ ] Schema inference automatica
- [ ] Salvataggio sessione (file aperti + query)
- [ ] Formattazione / pretty-print sezione selezionata

### Futuro (v1.0+)

- [ ] Plugin query engine custom
- [ ] Integrazione con file remoti (S3, HTTP)
- [ ] Diff tra due file JSON
- [ ] Visualizzazioni grafiche (chart su dati numerici)

---

## Struttura Progetto

```
JsonGUI/
├── src-tauri/                  # Backend Rust
│   ├── src/
│   │   ├── main.rs
│   │   ├── commands/           # Comandi Tauri (IPC)
│   │   │   ├── file.rs         # open_file, get_structure
│   │   │   ├── query.rs        # run_query, get_page
│   │   │   └── export.rs       # export_results
│   │   ├── engine/
│   │   │   ├── duckdb.rs       # DuckDB engine
│   │   │   ├── jaq.rs          # jaq engine
│   │   │   └── jsonpath.rs     # JSONPath engine
│   │   └── parser/
│   │       └── streaming.rs    # sonic-rs streaming parser
│   └── Cargo.toml
├── src/                        # Frontend Svelte
│   ├── lib/
│   │   ├── components/
│   │   │   ├── FileOpener.svelte
│   │   │   ├── TreeView.svelte
│   │   │   ├── QueryEditor.svelte
│   │   │   ├── ResultTable.svelte
│   │   │   └── StatusBar.svelte
│   │   └── stores/
│   │       ├── file.ts
│   │       └── query.ts
│   ├── App.svelte
│   └── main.ts
├── PLAN.md
├── package.json
└── tauri.conf.json
```

---

## Principi Tecnici Fondamentali

1. **Zero full-load** — il file JSON non viene mai caricato interamente in RAM
2. **Paginazione lato Rust** — il frontend riceve max 500 righe alla volta
3. **Thread separato per query** — la UI non si blocca mai durante l'elaborazione
4. **Indice lazy** — alla prima apertura si scansiona solo la struttura top-level
5. **IPC minimale** — si trasferisce solo ciò che la viewport mostra

---

## Priorità Sviluppo

```
Fase 1 — Scaffolding (3-4h)
  └── Setup Tauri 2 + Svelte 5 + Tailwind
  └── Layout base UI (sidebar + main + statusbar)
  └── Apertura file dialog nativo

Fase 2 — Core Engine (1-2 giorni)
  └── Integrazione DuckDB + comando run_query
  └── StreamingParser per tree view
  └── ResultTable con TanStack Virtual

Fase 3 — Query UX (1 giorno)
  └── Monaco Editor per SQL
  └── Selezione engine (SQL / jq / JSONPath)
  └── Paginazione e navigazione risultati

Fase 4 — Polish + Build (1 giorno)
  └── Export risultati
  └── Error handling e feedback utente
  └── Build cross-platform (GitHub Actions)
```

---

## Comandi di Sviluppo

```bash
# Setup iniziale
cargo install tauri-cli
npm create tauri-app@latest JsonGUI -- --template svelte-ts

# Dev
npm run tauri dev

# Build cross-platform
npm run tauri build
```

---

## Riferimenti

- [Tauri 2 Docs](https://tauri.app)
- [DuckDB JSON Functions](https://duckdb.org/docs/data/json)
- [sonic-rs crate](https://crates.io/crates/sonic-rs)
- [jaq crate](https://crates.io/crates/jaq)
- [TanStack Virtual](https://tanstack.com/virtual)
- [Monaco Editor](https://microsoft.github.io/monaco-editor/)

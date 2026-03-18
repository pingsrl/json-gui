use crate::json_index::{
    JsonIndex, NodeValue, ObjectSearchFilter as IndexObjectSearchFilter, ObjectSearchOperator,
    VisibleSliceRow,
};
use crate::schema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{BufReader, Read};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, State};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Wrapper attorno a un `Read` che emette una callback ogni volta che la percentuale avanza.
struct ProgressReader<R: Read, F: Fn(u8)> {
    inner: R,
    bytes_read: u64,
    total_bytes: u64,
    last_percent: u8,
    progress_cb: F,
}

impl<R: Read, F: Fn(u8)> ProgressReader<R, F> {
    fn new(inner: R, total_bytes: u64, progress_cb: F) -> Self {
        Self {
            inner,
            bytes_read: 0,
            total_bytes,
            last_percent: 0,
            progress_cb,
        }
    }
}

impl<R: Read, F: Fn(u8)> Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        let percent = if self.total_bytes > 0 {
            ((self.bytes_read * 100) / self.total_bytes).min(100) as u8
        } else {
            0
        };
        if percent != self.last_percent {
            (self.progress_cb)(percent);
            self.last_percent = percent;
        }
        Ok(n)
    }
}

pub struct AppState {
    // RwLock: letture concorrenti (search, get_children, get_path, get_expanded_slice)
    // senza bloccarsi a vicenda; write lock solo su open_file/open_from_string.
    pub index: Arc<RwLock<Option<JsonIndex>>>,
    pub initial_path: std::sync::Mutex<Option<String>>,
}

#[derive(Serialize, Clone)]
pub struct NodeDto {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub key: Option<String>,
    pub value_type: &'static str,
    pub value_preview: Cow<'static, str>,
    pub children_count: usize,
}

#[derive(Serialize)]
pub struct FileInfo {
    pub node_count: usize,
    pub size_bytes: usize,
    pub root_children: Vec<NodeDto>,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub node_id: u32,
    pub file_order: u32,
    pub path: String,
    pub key: Option<String>,
    pub value_preview: String,
    pub kind: &'static str,
    pub match_preview: Option<String>,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub target: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub exact_match: bool,
    pub max_results: usize,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct ObjectSearchFilterInput {
    pub path: String,
    pub operator: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Deserialize)]
pub struct ObjectSearchQuery {
    pub filters: Vec<ObjectSearchFilterInput>,
    pub key_case_sensitive: bool,
    pub value_case_sensitive: bool,
    pub max_results: usize,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct ExpandToResult {
    pub expansions: Vec<(u32, Vec<NodeDto>)>,
    pub path: String,
}


/// Tronca una stringa UTF-8 in modo sicuro al massimo `max_chars` caratteri.
fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Tipo del nodo come byte per il formato compatto IPC (get_expanded_slice).
/// 0=object, 1=array, 2=string, 3=number, 4=boolean, 5=null
fn node_type_byte(value: &NodeValue) -> u8 {
    match value {
        NodeValue::Object => 0,
        NodeValue::Array => 1,
        NodeValue::Str(_) => 2,
        NodeValue::Num(_) => 3,
        NodeValue::Bool(_) => 4,
        NodeValue::Null => 5,
    }
}

/// Preview testuale del valore di un nodo, riutilizzata da node_to_dto e dal
/// formato compatto di get_expanded_slice.
fn node_value_preview(value: &NodeValue, children_len: usize) -> Cow<'static, str> {
    match value {
        NodeValue::Object => {
            if children_len == 0 {
                Cow::Borrowed("{}")
            } else {
                Cow::Owned(format!("{{{} keys}}", children_len))
            }
        }
        NodeValue::Array => {
            if children_len == 0 {
                Cow::Borrowed("[]")
            } else {
                Cow::Owned(format!("[{} items]", children_len))
            }
        }
        NodeValue::Str(s) => {
            if s.chars().count() > 80 {
                Cow::Owned(format!("\"{}…\"", truncate_str(s, 80)))
            } else {
                Cow::Owned(format!("\"{}\"", s))
            }
        }
        NodeValue::Num(n) => Cow::Owned(n.to_string()),
        NodeValue::Bool(true) => Cow::Borrowed("true"),
        NodeValue::Bool(false) => Cow::Borrowed("false"),
        NodeValue::Null => Cow::Borrowed("null"),
    }
}

fn node_to_dto(index: &JsonIndex, id: u32) -> NodeDto {
    let node = &index.nodes[id as usize];
    let children_len = node.children_len as usize;
    let value_type: &'static str = match &node.value {
        NodeValue::Object => "object",
        NodeValue::Array => "array",
        NodeValue::Str(_) => "string",
        NodeValue::Num(_) => "number",
        NodeValue::Bool(_) => "boolean",
        NodeValue::Null => "null",
    };
    NodeDto {
        id,
        parent_id: node.parent,
        key: node.key.map(|kid| index.keys.get(kid).to_string()),
        value_type,
        value_preview: node_value_preview(&node.value, children_len),
        children_count: children_len,
    }
}

const STREAMING_THRESHOLD: u64 = 200 * 1024 * 1024; // 200 MB

#[tauri::command]
pub async fn open_file(
    path: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<FileInfo, String> {
    let path_clone = path.clone();
    let app_clone = app.clone();

    let (index, size_bytes) = tauri::async_runtime::spawn_blocking(move || {
        let file_size = std::fs::metadata(&path_clone).map(|m| m.len()).unwrap_or(0);

        let index = if file_size > STREAMING_THRESHOLD {
            // File >200MB: streaming con progress events
            let file = std::fs::File::open(&path_clone).map_err(|e| e.to_string())?;
            let reader = ProgressReader::new(BufReader::new(file), file_size, move |pct| {
                app_clone.emit("parse-progress", pct).ok();
            });
            JsonIndex::from_reader(reader)
        } else {
            JsonIndex::from_file(&path_clone)
        };

        index.map(|idx| (idx, file_size as usize))
    })
    .await
    .map_err(|e| e.to_string())??;

    let node_count = index.nodes.len();
    let root_children: Vec<NodeDto> = index
        .get_children_slice(index.root)
        .iter()
        .map(|&id| node_to_dto(&index, id))
        .collect();
    *state.index.write().unwrap() = Some(index);
    Ok(FileInfo {
        node_count,
        size_bytes,
        root_children,
    })
}

#[tauri::command]
pub async fn get_children(
    node_id: u32,
    state: State<'_, AppState>,
) -> Result<Vec<NodeDto>, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let children: Vec<NodeDto> = index
        .get_children_slice(node_id)
        .iter()
        .map(|&id| node_to_dto(index, id))
        .collect();
    Ok(children)
}

#[tauri::command]
pub async fn get_path(node_id: u32, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.get_path(node_id))
}

#[tauri::command]
pub async fn search(
    query: SearchQuery,
    state: State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let results = index.search(
        &query.text,
        &query.target,
        query.case_sensitive,
        query.regex,
        query.exact_match,
        query.max_results,
        query.path.as_deref(),
    );
    let dtos: Vec<SearchResult> = results
        .into_iter()
        .map(|(id, path)| {
            let node = &index.nodes[id as usize];
            let value_preview = match &node.value {
                NodeValue::Str(s) => format!("\"{}\"", truncate_str(s, 60)),
                NodeValue::Num(n) => n.to_string(),
                NodeValue::Bool(b) => b.to_string(),
                NodeValue::Null => "null".to_string(),
                NodeValue::Object => "[object]".to_string(),
                NodeValue::Array => "[array]".to_string(),
            };
            SearchResult {
                node_id: id,
                file_order: node.preorder_index,
                path,
                key: node.key.map(|kid| index.keys.get(kid).to_string()),
                value_preview,
                kind: "node",
                match_preview: None,
            }
        })
        .collect();
    Ok(dtos)
}

fn parse_object_search_operator(operator: &str) -> Option<ObjectSearchOperator> {
    match operator {
        "contains" => Some(ObjectSearchOperator::Contains),
        "equals" => Some(ObjectSearchOperator::Equals),
        "regex" => Some(ObjectSearchOperator::Regex),
        "exists" => Some(ObjectSearchOperator::Exists),
        _ => None,
    }
}

fn build_object_match_preview(filters: &[ObjectSearchFilterInput]) -> String {
    filters
        .iter()
        .map(|filter| match filter.operator.as_str() {
            "exists" => format!("{} exists", filter.path.trim()),
            "equals" => format!(
                "{} = {}",
                filter.path.trim(),
                filter.value.as_deref().unwrap_or("").trim()
            ),
            "regex" => format!(
                "{} ~= {}",
                filter.path.trim(),
                filter.value.as_deref().unwrap_or("").trim()
            ),
            _ => format!(
                "{} contains {}",
                filter.path.trim(),
                filter.value.as_deref().unwrap_or("").trim()
            ),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[tauri::command]
pub async fn search_objects(
    query: ObjectSearchQuery,
    state: State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;

    let filters: Vec<IndexObjectSearchFilter> = query
        .filters
        .iter()
        .map(|filter| {
            let operator = parse_object_search_operator(&filter.operator)
                .ok_or_else(|| format!("Operatore non supportato: {}", filter.operator))?;
            Ok(IndexObjectSearchFilter {
                path: filter.path.clone(),
                operator,
                value: filter.value.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let match_preview = build_object_match_preview(&query.filters);
    let ids = index.search_objects(
        &filters,
        query.key_case_sensitive,
        query.value_case_sensitive,
        query.max_results,
        query.path.as_deref(),
    );
    let dtos = ids
        .into_iter()
        .map(|id| {
            let node = &index.nodes[id as usize];
            let children_len = node.children_len as usize;
            // Inline value_preview per oggetti: evita di costruire NodeDto completo
            let value_preview = if children_len == 0 {
                "{}".to_string()
            } else {
                format!("{{{} keys}}", children_len)
            };
            SearchResult {
                node_id: id,
                file_order: node.preorder_index,
                path: index.get_path(id),
                key: node.key.map(|kid| index.keys.get(kid).to_string()),
                value_preview,
                kind: "object",
                match_preview: Some(match_preview.clone()),
            }
        })
        .collect();
    Ok(dtos)
}

#[tauri::command]
pub async fn suggest_property_paths(
    prefix: String,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.suggest_property_paths(&prefix, limit))
}

/// Restituisce per ogni antenato di node_id (da root a parent) la coppia (id, figli),
/// piu' il path del nodo target, così il frontend può espandere l'intero path
/// e ottenere il path in un singolo IPC call.
#[tauri::command]
pub async fn expand_to(node_id: u32, state: State<'_, AppState>) -> Result<ExpandToResult, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;

    let mut chain: Vec<u32> = Vec::new();
    let mut current = node_id;
    loop {
        let node = &index.nodes[current as usize];
        match node.parent {
            Some(parent_id) => {
                chain.push(parent_id);
                current = parent_id;
            }
            None => break,
        }
    }
    chain.reverse(); // da root verso il parent diretto

    let expansions: Vec<(u32, Vec<NodeDto>)> = chain
        .into_iter()
        .map(|ancestor_id| {
            let children = index
                .get_children_slice(ancestor_id)
                .iter()
                .map(|&child_id| node_to_dto(index, child_id))
                .collect();
            (ancestor_id, children)
        })
        .collect();

    let path = index.get_path(node_id);

    Ok(ExpandToResult { expansions, path })
}

/// Formato compatto per get_expanded_slice: tuple invece di oggetti JSON con nomi
/// di campo ripetuti. Ogni row è [id, parent_id, key_idx, type, preview, n_children, depth]
/// con key_idx come indice nel pool locale di chiavi deduplicate (-1 = nessuna chiave).
/// Risparmio tipico: ~70% vs ExpandedSliceResult con NodeDto a campi nominati.
#[derive(Serialize)]
pub struct CompactExpandedSliceResult {
    pub offset: usize,
    pub total_count: usize,
    /// Pool locale di chiavi uniche per questo slice; indexato da key_idx nelle row.
    pub key_pool: Vec<String>,
    /// [id, parent_id (-1=root), key_idx (-1=none), type_byte, preview, children_count, depth]
    pub rows: Vec<(u32, i32, i32, u8, String, u32, u32)>,
}

#[tauri::command]
pub async fn get_expanded_slice(
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<CompactExpandedSliceResult, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let slice = index.get_expanded_slice(offset, limit);

    // Pool locale di chiavi per questo slice: evita di ripetere la stessa stringa
    // per ogni nodo che condivide il nome di campo (es. "id", "name", …).
    let mut key_pool: Vec<String> = Vec::new();
    let mut key_pool_ids: Vec<u32> = Vec::new(); // parallel: key_pool_ids[i] = string-pool id

    let mut rows = Vec::with_capacity(slice.len());
    for VisibleSliceRow { id, depth } in slice {
        let node = &index.nodes[id as usize];
        let children_len = node.children_len as usize;

        let key_idx: i32 = match node.key {
            None => -1,
            Some(kid) => {
                // ricerca lineare sul pool locale (solitamente < 200 entry → cache-friendly)
                match key_pool_ids.iter().position(|&k| k == kid) {
                    Some(pos) => pos as i32,
                    None => {
                        let pos = key_pool.len() as i32;
                        key_pool.push(index.keys.get(kid).to_string());
                        key_pool_ids.push(kid);
                        pos
                    }
                }
            }
        };

        rows.push((
            id,
            node.parent.map(|p| p as i32).unwrap_or(-1),
            key_idx,
            node_type_byte(&node.value),
            node_value_preview(&node.value, children_len).into_owned(),
            node.children_len,
            depth as u32,
        ));
    }

    Ok(CompactExpandedSliceResult {
        offset,
        total_count: index.expanded_visible_count(),
        key_pool,
        rows,
    })
}

// Primo chunk piccolo per mostrare subito l'espansione in UI, poi chunk più grandi
// per mantenere basso l'overhead IPC sul resto dell'operazione.
const EXPAND_FIRST_CHUNK_NODE_TARGET: usize = 1_000;
const EXPAND_CHUNK_NODE_TARGET: usize = 10_000;
const EXPAND_CHUNK_MAX_LATENCY_MS: u64 = 250;
const EXPAND_CHUNK_MAX_LATENCY: Duration = Duration::from_millis(EXPAND_CHUNK_MAX_LATENCY_MS);
const EXPAND_FIRST_EMIT_PAUSE_MS: u64 = 12;
const EXPAND_EMIT_PAUSE_MS: u64 = 1;

#[derive(Serialize, Clone)]
pub struct ExpandChunk {
    pub expansions: Vec<(u32, Vec<NodeDto>)>,
    pub progress: u8,
}

fn expand_chunk_target(first_chunk_sent: bool) -> usize {
    if first_chunk_sent {
        EXPAND_CHUNK_NODE_TARGET
    } else {
        EXPAND_FIRST_CHUNK_NODE_TARGET
    }
}

fn should_flush_expand_chunk(
    chunk_nodes: usize,
    first_chunk_sent: bool,
    elapsed: Duration,
) -> bool {
    chunk_nodes > 0
        && (chunk_nodes >= expand_chunk_target(first_chunk_sent)
            || elapsed >= EXPAND_CHUNK_MAX_LATENCY)
}

fn flush_expand_chunk(
    tx: &mpsc::Sender<ExpandChunk>,
    chunk: &mut Vec<(u32, Vec<NodeDto>)>,
    total_sent: u64,
    total_nodes: u64,
) -> bool {
    let progress = ((total_sent * 100) / total_nodes.max(1)).min(99) as u8;
    tx.blocking_send(ExpandChunk {
        expansions: std::mem::take(chunk),
        progress,
    })
    .is_ok()
}

fn walk_expandable_nodes_breadth_first<F>(index: &JsonIndex, mut visit: F) -> bool
where
    F: FnMut(u32, &[u32]) -> bool,
{
    let mut queue: VecDeque<u32> = VecDeque::new();
    queue.extend(index.get_children_slice(index.root).iter().copied());

    while let Some(node_id) = queue.pop_front() {
        let children_slice = index.get_children_slice(node_id);
        if children_slice.is_empty() {
            continue;
        }
        if !visit(node_id, children_slice) {
            return false;
        }
        queue.extend(children_slice.iter().copied());
    }

    true
}

#[tauri::command]
pub async fn expand_all(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let index_arc = Arc::clone(&state.index);
    let (tx, mut rx) = mpsc::channel::<ExpandChunk>(32);

    // BFS orizzontale + DTO build su un thread dedicato.
    // Il Mutex è tenuto solo dentro spawn_blocking, non attraverso await.
    tauri::async_runtime::spawn_blocking(move || {
        let guard = index_arc.read().unwrap();
        let index = match guard.as_ref() {
            Some(i) => i,
            None => return,
        };

        let total_nodes = index.nodes.len() as u64;
        let mut total_sent: u64 = 0;
        let mut chunk: Vec<(u32, Vec<NodeDto>)> = Vec::new();
        let mut chunk_nodes: usize = 0;
        let mut first_chunk_sent = false;
        let mut last_flush = Instant::now();
        let completed = walk_expandable_nodes_breadth_first(index, |node_id, children_slice| {
            let n = children_slice.len() as u64;
            let children: Vec<NodeDto> = children_slice
                .iter()
                .map(|&id| node_to_dto(index, id))
                .collect();
            chunk.push((node_id, children));
            chunk_nodes += n as usize;
            total_sent += n;
            if !first_chunk_sent {
                if !flush_expand_chunk(&tx, &mut chunk, total_sent, total_nodes) {
                    return false; // receiver droppato: operazione annullata
                }
                chunk_nodes = 0;
                first_chunk_sent = true;
                last_flush = Instant::now();
                return true;
            }
            if should_flush_expand_chunk(chunk_nodes, first_chunk_sent, last_flush.elapsed()) {
                if !flush_expand_chunk(&tx, &mut chunk, total_sent, total_nodes) {
                    return false; // receiver droppato: operazione annullata
                }
                chunk_nodes = 0;
                last_flush = Instant::now();
            }
            true
        });

        if !completed {
            return;
        }

        if !chunk.is_empty() {
            let progress = ((total_sent * 100) / total_nodes.max(1)).min(99) as u8;
            let _ = tx.blocking_send(ExpandChunk {
                expansions: chunk,
                progress,
            });
        }
        // tx droppato qui → rx.recv() restituisce None → loop termina
    });

    // Emette i chunk verso il frontend man mano che arrivano.
    // Il thread Tokio è libero (await) → la UI riceve gli eventi in tempo reale.
    let mut emitted_chunks = 0usize;
    while let Some(chunk) = rx.recv().await {
        app.emit("expand-chunk", chunk).ok();
        let pause_ms = if emitted_chunks == 0 {
            EXPAND_FIRST_EMIT_PAUSE_MS
        } else {
            EXPAND_EMIT_PAUSE_MS
        };
        emitted_chunks += 1;
        sleep(Duration::from_millis(pause_ms)).await;
    }
    app.emit("expand-done", ()).ok();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_expand_chunk_has_smaller_target() {
        assert_eq!(expand_chunk_target(false), EXPAND_FIRST_CHUNK_NODE_TARGET);
        assert_eq!(expand_chunk_target(true), EXPAND_CHUNK_NODE_TARGET);
        assert!(expand_chunk_target(false) < expand_chunk_target(true));
    }

    #[test]
    fn immediate_first_parent_flush_is_supported() {
        let (tx, mut rx) = mpsc::channel::<ExpandChunk>(1);
        let mut chunk = vec![(1, Vec::new())];

        assert!(flush_expand_chunk(&tx, &mut chunk, 10, 100));
        assert!(chunk.is_empty());

        let emitted = rx.blocking_recv().expect("missing chunk");
        assert_eq!(emitted.expansions.len(), 1);
        assert_eq!(emitted.progress, 10);
    }

    #[test]
    fn flushes_when_node_budget_is_reached() {
        assert!(!should_flush_expand_chunk(
            EXPAND_FIRST_CHUNK_NODE_TARGET - 1,
            false,
            Duration::from_millis(0),
        ));
        assert!(should_flush_expand_chunk(
            EXPAND_FIRST_CHUNK_NODE_TARGET,
            false,
            Duration::from_millis(0),
        ));
        assert!(should_flush_expand_chunk(
            EXPAND_CHUNK_NODE_TARGET,
            true,
            Duration::from_millis(0),
        ));
    }

    #[test]
    fn flushes_on_latency_even_with_small_chunks() {
        assert!(!should_flush_expand_chunk(
            1,
            false,
            Duration::from_millis(EXPAND_CHUNK_MAX_LATENCY_MS - 1),
        ));
        assert!(should_flush_expand_chunk(
            1,
            false,
            Duration::from_millis(EXPAND_CHUNK_MAX_LATENCY_MS),
        ));
        assert!(!should_flush_expand_chunk(
            0,
            false,
            Duration::from_millis(EXPAND_CHUNK_MAX_LATENCY_MS),
        ));
    }

    #[test]
    fn walks_expandable_nodes_horizontally() {
        let index = JsonIndex::from_str(
            r#"{
                "a": {
                    "a1": { "leaf": 1 },
                    "a2": 2
                },
                "b": {
                    "b1": 3
                },
                "c": 4
            }"#,
        )
        .unwrap();

        let mut order = Vec::new();
        let completed = walk_expandable_nodes_breadth_first(&index, |node_id, _children| {
            order.push(index.get_path(node_id));
            true
        });

        assert!(completed);
        assert_eq!(order, vec!["$.a", "$.b", "$.a.a1"]);
    }
}

#[tauri::command]
pub async fn get_raw(node_id: u32, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.build_raw(node_id))
}

#[tauri::command]
pub async fn export_types(lang: String, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.index.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let result = match lang.as_str() {
        "typescript" => schema::generate_typescript(index),
        "zod" => schema::generate_zod(index),
        "rust" => schema::generate_rust(index),
        "go" => schema::generate_go(index),
        "python" => schema::generate_python(index),
        "json-schema" => schema::generate_json_schema(index),
        other => return Err(format!("Linguaggio non supportato: {}", other)),
    };
    Ok(result)
}

#[tauri::command]
pub fn get_initial_path(state: State<'_, AppState>) -> Option<String> {
    state.initial_path.lock().unwrap().take()
}

#[tauri::command]
pub async fn open_from_string(
    content: String,
    state: State<'_, AppState>,
) -> Result<FileInfo, String> {
    let size_bytes = content.len();
    let index = tauri::async_runtime::spawn_blocking(move || JsonIndex::from_str(&content))
        .await
        .map_err(|e| e.to_string())??;

    let node_count = index.nodes.len();
    let root_children: Vec<NodeDto> = index
        .get_children_slice(index.root)
        .iter()
        .map(|&id| node_to_dto(&index, id))
        .collect();
    *state.index.write().unwrap() = Some(index);
    Ok(FileInfo {
        node_count,
        size_bytes,
        root_children,
    })
}

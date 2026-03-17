use crate::json_index::{JsonIndex, NodeValue};
use crate::schema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{BufReader, Read};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use tokio::sync::mpsc;

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
        Self { inner, bytes_read: 0, total_bytes, last_percent: 0, progress_cb }
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
    pub index: Arc<Mutex<Option<JsonIndex>>>,
    pub initial_path: Mutex<Option<String>>,
}

#[derive(Serialize, Clone)]
pub struct NodeDto {
    pub id: u32,
    pub key: Option<String>,
    pub value_type: &'static str,
    pub value_preview: Cow<'static, str>,
    pub has_children: bool,
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
    pub path: String,
    pub key: Option<String>,
    pub value_preview: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub target: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub exact_match: bool,
    pub max_results: usize,
}

#[derive(Serialize)]
pub struct ExpandToResult {
    pub expansions: Vec<(u32, Vec<NodeDto>)>,
    pub path: String,
}


/// Tronca una stringa UTF-8 in modo sicuro al massimo `max_chars` caratteri.
fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn node_to_dto(index: &JsonIndex, id: u32) -> NodeDto {
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
                Cow::Owned(format!("\"{}…\"", truncate_str(s, 80)))
            } else {
                Cow::Owned(format!("\"{}\"", s))
            },
        ),
        NodeValue::Num(n) => ("number", Cow::Owned(n.to_string())),
        NodeValue::Bool(true) => ("boolean", Cow::Borrowed("true")),
        NodeValue::Bool(false) => ("boolean", Cow::Borrowed("false")),
        NodeValue::Null => ("null", Cow::Borrowed("null")),
    };
    NodeDto {
        id,
        key: node.key.map(|kid| index.keys.get(kid).to_string()),
        value_type,
        value_preview,
        has_children,
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
        let file_size = std::fs::metadata(&path_clone)
            .map(|m| m.len())
            .unwrap_or(0);

        let index = if file_size > STREAMING_THRESHOLD {
            // File >200MB: streaming con progress events
            let file =
                std::fs::File::open(&path_clone).map_err(|e| e.to_string())?;
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
    *state.index.lock().unwrap() = Some(index);
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
    let guard = state.index.lock().unwrap();
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
    let guard = state.index.lock().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.get_path(node_id))
}

#[tauri::command]
pub async fn search(
    query: SearchQuery,
    state: State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let guard = state.index.lock().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let results = index.search(
        &query.text,
        &query.target,
        query.case_sensitive,
        query.regex,
        query.exact_match,
        query.max_results,
    );
    let dtos: Vec<SearchResult> = results
        .into_iter()
        .map(|(id, path)| {
            let node = &index.nodes[id as usize];
            let value_preview = match &node.value {
                NodeValue::Str(s) => format!(
                    "\"{}\"",
                    truncate_str(s, 60)
                ),
                NodeValue::Num(n) => n.to_string(),
                NodeValue::Bool(b) => b.to_string(),
                NodeValue::Null => "null".to_string(),
                NodeValue::Object => "[object]".to_string(),
                NodeValue::Array => "[array]".to_string(),
            };
            SearchResult {
                node_id: id,
                path,
                key: node.key.map(|kid| index.keys.get(kid).to_string()),
                value_preview,
            }
        })
        .collect();
    Ok(dtos)
}

/// Restituisce per ogni antenato di node_id (da root a parent) la coppia (id, figli),
/// piu' il path del nodo target, così il frontend può espandere l'intero path
/// e ottenere il path in un singolo IPC call.
#[tauri::command]
pub async fn expand_to(
    node_id: u32,
    state: State<'_, AppState>,
) -> Result<ExpandToResult, String> {
    let guard = state.index.lock().unwrap();
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

// 10k nodi/chunk → ~100 chiamate IPC per 1M nodi (vs 1000 con chunk=1k)
// Riduce l'overhead IPC 10x mantenendo feedback visivo ogni ~500ms
const EXPAND_CHUNK_SIZE: usize = 10_000;

#[derive(Serialize, Clone)]
pub struct ExpandChunk {
    pub expansions: Vec<(u32, Vec<NodeDto>)>,
    pub progress: u8,
}

#[tauri::command]
pub async fn expand_all(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let index_arc = Arc::clone(&state.index);
    let (tx, mut rx) = mpsc::channel::<ExpandChunk>(32);

    // BFS + DTO build su un thread dedicato (non blocca il runtime Tokio).
    // Il Mutex è tenuto solo dentro spawn_blocking, non attraverso await.
    tauri::async_runtime::spawn_blocking(move || {
        let guard = index_arc.lock().unwrap();
        let index = match guard.as_ref() {
            Some(i) => i,
            None => return,
        };

        let total_nodes = index.nodes.len() as u64;
        let mut total_sent: u64 = 0;
        let mut chunk: Vec<(u32, Vec<NodeDto>)> = Vec::with_capacity(EXPAND_CHUNK_SIZE);
        let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();

        for &child_id in index.get_children_slice(index.root) {
            queue.push_back(child_id);
        }

        while let Some(node_id) = queue.pop_front() {
            let children_slice = index.get_children_slice(node_id);
            if children_slice.is_empty() {
                continue;
            }
            let n = children_slice.len() as u64;
            let children: Vec<NodeDto> =
                children_slice.iter().map(|&id| node_to_dto(index, id)).collect();
            for &child_id in children_slice {
                queue.push_back(child_id);
            }
            chunk.push((node_id, children));
            total_sent += n;
            if chunk.len() >= EXPAND_CHUNK_SIZE {
                let progress = ((total_sent * 100) / total_nodes.max(1)).min(99) as u8;
                if tx
                    .blocking_send(ExpandChunk {
                        expansions: std::mem::take(&mut chunk),
                        progress,
                    })
                    .is_err()
                {
                    return; // receiver droppato: operazione annullata
                }
                chunk = Vec::with_capacity(EXPAND_CHUNK_SIZE);
            }
        }

        if !chunk.is_empty() {
            let _ = tx.blocking_send(ExpandChunk { expansions: chunk, progress: 99 });
        }
        // tx droppato qui → rx.recv() restituisce None → loop termina
    });

    // Emette i chunk verso il frontend man mano che arrivano.
    // Il thread Tokio è libero (await) → la UI riceve gli eventi in tempo reale.
    while let Some(chunk) = rx.recv().await {
        app.emit("expand-chunk", chunk).ok();
    }
    app.emit("expand-done", ()).ok();

    Ok(())
}

#[tauri::command]
pub async fn get_raw(node_id: u32, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.index.lock().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.build_raw(node_id))
}

#[tauri::command]
pub async fn export_types(
    lang: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let guard = state.index.lock().unwrap();
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
    let index = tauri::async_runtime::spawn_blocking(move || {
        JsonIndex::from_str(&content)
    })
    .await
    .map_err(|e| e.to_string())??;

    let node_count = index.nodes.len();
    let root_children: Vec<NodeDto> = index
        .get_children_slice(index.root)
        .iter()
        .map(|&id| node_to_dto(&index, id))
        .collect();
    *state.index.lock().unwrap() = Some(index);
    Ok(FileInfo {
        node_count,
        size_bytes,
        root_children,
    })
}

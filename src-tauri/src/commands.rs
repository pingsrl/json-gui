use crate::json_index::{JsonIndex, NodeValue};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read};
use std::sync::Mutex;
use tauri::{Emitter, State};

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
    pub index: Mutex<Option<JsonIndex>>,
}

#[derive(Serialize, Clone)]
pub struct NodeDto {
    pub id: u32,
    pub key: Option<String>,
    pub value_type: String,
    pub value_preview: String,
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
    pub max_results: usize,
}

fn node_to_dto(index: &JsonIndex, id: u32) -> NodeDto {
    let node = &index.nodes[id as usize];
    let (value_type, value_preview) = match &node.value {
        NodeValue::Object => (
            "object".to_string(),
            if node.children.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{} keys}}", node.children.len())
            },
        ),
        NodeValue::Array => (
            "array".to_string(),
            if node.children.is_empty() {
                "[]".to_string()
            } else {
                format!("[{} items]", node.children.len())
            },
        ),
        NodeValue::Str(s) => (
            "string".to_string(),
            if s.len() > 80 {
                format!("\"{}…\"", &s[..80])
            } else {
                format!("\"{}\"", s)
            },
        ),
        NodeValue::Num(n) => ("number".to_string(), n.to_string()),
        NodeValue::Bool(b) => ("boolean".to_string(), b.to_string()),
        NodeValue::Null => ("null".to_string(), "null".to_string()),
    };
    NodeDto {
        id,
        key: node.key.clone(),
        value_type,
        value_preview,
        has_children: !node.children.is_empty(),
        children_count: node.children.len(),
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
    let root_children: Vec<NodeDto> = index.nodes[index.root as usize]
        .children
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
    let children: Vec<NodeDto> = index.nodes[node_id as usize]
        .children
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
        query.max_results,
    );
    let dtos: Vec<SearchResult> = results
        .into_iter()
        .map(|(id, path)| {
            let node = &index.nodes[id as usize];
            let value_preview = match &node.value {
                NodeValue::Str(s) => format!(
                    "\"{}\"",
                    if s.len() > 60 { &s[..60] } else { s }
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
                key: node.key.clone(),
                value_preview,
            }
        })
        .collect();
    Ok(dtos)
}

/// Restituisce per ogni antenato di node_id (da root a parent) la coppia (id, figli),
/// così il frontend può espandere l'intero path in un singolo IPC call.
#[tauri::command]
pub async fn expand_to(
    node_id: u32,
    state: State<'_, AppState>,
) -> Result<Vec<(u32, Vec<NodeDto>)>, String> {
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

    let result: Vec<(u32, Vec<NodeDto>)> = chain
        .into_iter()
        .map(|ancestor_id| {
            let children = index.nodes[ancestor_id as usize]
                .children
                .iter()
                .map(|&child_id| node_to_dto(index, child_id))
                .collect();
            (ancestor_id, children)
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn get_raw(node_id: u32, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.index.lock().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(build_raw(index, node_id))
}

fn build_raw(index: &JsonIndex, id: u32) -> String {
    let node = &index.nodes[id as usize];
    match &node.value {
        NodeValue::Object => {
            let fields: Vec<String> = node
                .children
                .iter()
                .map(|&child_id| {
                    let child = &index.nodes[child_id as usize];
                    let key = child.key.as_deref().unwrap_or("");
                    format!("\"{}\":{}", key, build_raw(index, child_id))
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        NodeValue::Array => {
            let items: Vec<String> = node
                .children
                .iter()
                .map(|&child_id| build_raw(index, child_id))
                .collect();
            format!("[{}]", items.join(","))
        }
        NodeValue::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        NodeValue::Num(n) => n.to_string(),
        NodeValue::Bool(b) => b.to_string(),
        NodeValue::Null => "null".to_string(),
    }
}

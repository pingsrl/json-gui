use crate::json_index::{JsonIndex, NodeValue};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

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

#[tauri::command]
pub async fn open_file(path: String, state: State<'_, AppState>) -> Result<FileInfo, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let size_bytes = content.len();
    let index = JsonIndex::from_str(&content)?;
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

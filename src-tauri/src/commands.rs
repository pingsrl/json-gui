use crate::json_index::{
    JsonIndex, NodeKey, NodeKind, ObjectSearchFilter as IndexObjectSearchFilter,
    ObjectSearchOperator, VisibleSliceRow,
};
use crate::schema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tauri::{Emitter, Manager, State, WindowEvent};

pub struct AppState {
    /// Map window-label → JSON index for that window.
    /// The outer RwLock protects the map; the inner RwLock protects the per-window index,
    /// allowing concurrent reads (search, get_children, …) on the same window.
    pub windows: RwLock<HashMap<String, Arc<RwLock<Option<JsonIndex>>>>>,
    pub initial_path: std::sync::Mutex<Option<String>>,
    /// Raw JSON pre-loaded for a window opened via "Open in new window".
    /// Key = window label; consumed exactly once by get_pending_content.
    pub pending_content: std::sync::Mutex<HashMap<String, String>>,
    pub runtime_monitor: Mutex<RuntimeMonitor>,
}

impl AppState {
    /// Returns (or creates) the Arc<RwLock<Option<JsonIndex>>> for the given window.
    pub fn window_index(&self, label: &str) -> Arc<RwLock<Option<JsonIndex>>> {
        {
            let read = self.windows.read().unwrap();
            if let Some(idx) = read.get(label) {
                return Arc::clone(idx);
            }
        }
        let mut write = self.windows.write().unwrap();
        Arc::clone(
            write
                .entry(label.to_string())
                .or_insert_with(|| Arc::new(RwLock::new(None))),
        )
    }

    /// Removes the index associated with a window (called on window destruction).
    pub fn remove_window(&self, label: &str) {
        self.windows.write().unwrap().remove(label);
        self.pending_content.lock().unwrap().remove(label);
    }
}

#[derive(Clone, Copy)]
struct RuntimeSample {
    resident_bytes: u64,
    total_cpu_ns: u64,
    captured_at: Instant,
}

impl RuntimeSample {
    fn cpu_percent_since(&self, previous: RuntimeSample) -> f32 {
        let elapsed = self.captured_at.duration_since(previous.captured_at);
        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= f64::EPSILON {
            return 0.0;
        }
        let cpu_delta_ns = self.total_cpu_ns.saturating_sub(previous.total_cpu_ns) as f64;
        ((cpu_delta_ns / 1_000_000_000.0) / elapsed_secs * 100.0) as f32
    }
}

pub struct RuntimeMonitor {
    last_sample: Option<RuntimeSample>,
}

impl RuntimeMonitor {
    pub fn new() -> Self {
        Self {
            last_sample: read_runtime_sample().ok(),
        }
    }

    fn snapshot(&mut self) -> Result<RuntimeStats, String> {
        let sample = read_runtime_sample()?;
        let cpu_percent = self
            .last_sample
            .map(|previous| sample.cpu_percent_since(previous))
            .unwrap_or(0.0);
        self.last_sample = Some(sample);
        Ok(RuntimeStats {
            resident_bytes: sample.resident_bytes,
            cpu_percent,
        })
    }
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
    pub root_node: NodeDto,
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
    /// Arc<str> instead of String: all results from the same search_objects call
    /// share the same string via reference-counting (O(1) clone).
    pub match_preview: Option<Arc<str>>,
}

#[derive(Serialize)]
pub struct RuntimeStats {
    pub resident_bytes: u64,
    pub cpu_percent: f32,
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
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub dot_all: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSearchFilterInput {
    pub path: String,
    pub operator: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub regex_case_insensitive: bool,
    #[serde(default)]
    pub regex_multiline: bool,
    #[serde(default)]
    pub regex_dot_all: bool,
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

/// Safely truncates a UTF-8 string to at most `max_chars` characters.
fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Node type as a byte for the compact IPC format (get_expanded_slice).
/// 0=object, 1=array, 2=string, 3=number, 4=boolean, 5=null
fn node_type_byte(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::Object => 0,
        NodeKind::Array => 1,
        NodeKind::Str => 2,
        NodeKind::Num => 3,
        NodeKind::Bool => 4,
        NodeKind::Null => 5,
    }
}

fn node_key_string(index: &JsonIndex, node_id: u32) -> Option<String> {
    match index.nodes[node_id as usize].key()? {
        NodeKey::String(kid) => Some(index.keys.get(kid).to_string()),
        NodeKey::ArrayIndex(idx) => Some(idx.to_string()),
    }
}

/// Text preview of a node's value, shared by node_to_dto and the compact
/// format of get_expanded_slice.
fn node_value_preview(index: &JsonIndex, id: u32) -> Cow<'static, str> {
    let node = &index.nodes[id as usize];
    let children_len = index.children_len(id) as usize;
    match node.kind() {
        NodeKind::Object => {
            if children_len == 0 {
                Cow::Borrowed("{}")
            } else {
                Cow::Owned(format!("{{{} keys}}", children_len))
            }
        }
        NodeKind::Array => {
            if children_len == 0 {
                Cow::Borrowed("[]")
            } else {
                Cow::Owned(format!("[{} items]", children_len))
            }
        }
        NodeKind::Str => {
            let s = index.str_val_of_node(node);
            let truncated = truncate_str(s, 80);
            if truncated.len() < s.len() {
                Cow::Owned(format!("\"{}…\"", truncated))
            } else {
                Cow::Owned(format!("\"{}\"", s))
            }
        }
        NodeKind::Num => Cow::Owned(index.number_to_string(id)),
        NodeKind::Bool => {
            if node.value_data != 0 {
                Cow::Borrowed("true")
            } else {
                Cow::Borrowed("false")
            }
        }
        NodeKind::Null => Cow::Borrowed("null"),
    }
}

fn node_to_dto(index: &JsonIndex, id: u32) -> NodeDto {
    let node = &index.nodes[id as usize];
    let children_len = index.children_len(id) as usize;
    let value_type: &'static str = match node.kind() {
        NodeKind::Object => "object",
        NodeKind::Array => "array",
        NodeKind::Str => "string",
        NodeKind::Num => "number",
        NodeKind::Bool => "boolean",
        NodeKind::Null => "null",
    };
    NodeDto {
        id,
        parent_id: index.parent_of(id),
        key: node_key_string(index, id),
        value_type,
        value_preview: node_value_preview(index, id),
        children_count: children_len,
    }
}

const PROGRESS_EVENT_THRESHOLD: u64 = 200 * 1024 * 1024; // 200 MB
const EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT: usize = 1_000;

#[tauri::command]
pub async fn open_file(
    path: String,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<FileInfo, String> {
    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
    if size_bytes as u64 > PROGRESS_EVENT_THRESHOLD {
        let _ = webview_window.emit("parse-progress", 0u8);
    }

    let index = tauri::async_runtime::spawn_blocking(move || JsonIndex::from_file(&path))
        .await
        .map_err(|e| e.to_string())??;

    if size_bytes as u64 > PROGRESS_EVENT_THRESHOLD {
        let _ = webview_window.emit("parse-progress", 100u8);
    }

    let node_count = index.nodes.len();
    let root_node = node_to_dto(&index, index.root);
    // Cap to the same page size the frontend uses for large nodes so that
    // we never serialize / transmit millions of NodeDtos in one shot.
    let root_children: Vec<NodeDto> = index
        .children_iter(index.root)
        .take(EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT)
        .map(|id| node_to_dto(&index, id))
        .collect();
    *state.window_index(webview_window.label()).write().unwrap() = Some(index);
    Ok(FileInfo {
        node_count,
        size_bytes,
        root_node,
        root_children,
    })
}

#[tauri::command]
pub async fn get_children(
    node_id: u32,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<NodeDto>, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let children: Vec<NodeDto> = index
        .children_iter(node_id)
        .map(|id| node_to_dto(index, id))
        .collect();
    Ok(children)
}

#[tauri::command]
pub async fn get_children_page(
    node_id: u32,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<NodeDto>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let children: Vec<NodeDto> = index
        .children_iter(node_id)
        .skip(offset)
        .take(limit)
        .map(|id| node_to_dto(index, id))
        .collect();
    Ok(children)
}

#[tauri::command]
pub fn get_runtime_stats(state: State<'_, AppState>) -> Result<RuntimeStats, String> {
    state.runtime_monitor.lock().unwrap().snapshot()
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct RusageInfoV4 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
    ri_child_user_time: u64,
    ri_child_system_time: u64,
    ri_child_pkg_idle_wkups: u64,
    ri_child_interrupt_wkups: u64,
    ri_child_pageins: u64,
    ri_child_elapsed_abstime: u64,
    ri_diskio_bytesread: u64,
    ri_diskio_byteswritten: u64,
    ri_cpu_time_qos_default: u64,
    ri_cpu_time_qos_maintenance: u64,
    ri_cpu_time_qos_background: u64,
    ri_cpu_time_qos_utility: u64,
    ri_cpu_time_qos_legacy: u64,
    ri_cpu_time_qos_user_initiated: u64,
    ri_cpu_time_qos_user_interactive: u64,
    ri_billed_system_time: u64,
    ri_serviced_system_time: u64,
    ri_logical_writes: u64,
    ri_lifetime_max_phys_footprint: u64,
    ri_instructions: u64,
    ri_cycles: u64,
    ri_billed_energy: u64,
    ri_serviced_energy: u64,
    ri_interval_max_phys_footprint: u64,
    ri_runnable_time: u64,
}

#[cfg(target_os = "macos")]
const RUSAGE_INFO_V4: std::ffi::c_int = 4;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_pid_rusage(
        pid: std::ffi::c_int,
        flavor: std::ffi::c_int,
        buffer: *mut core::ffi::c_void,
    ) -> std::ffi::c_int;
}

#[cfg(target_os = "macos")]
fn read_runtime_sample() -> Result<RuntimeSample, String> {
    let mut usage = std::mem::MaybeUninit::<RusageInfoV4>::zeroed();
    let result = unsafe {
        proc_pid_rusage(
            std::process::id() as std::ffi::c_int,
            RUSAGE_INFO_V4,
            usage.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return Err(format!(
            "proc_pid_rusage failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let usage = unsafe { usage.assume_init() };
    Ok(RuntimeSample {
        resident_bytes: usage.ri_resident_size,
        total_cpu_ns: usage.ri_user_time.saturating_add(usage.ri_system_time),
        captured_at: Instant::now(),
    })
}

#[cfg(not(target_os = "macos"))]
fn read_runtime_sample() -> Result<RuntimeSample, String> {
    Ok(RuntimeSample {
        resident_bytes: 0,
        total_cpu_ns: 0,
        captured_at: Instant::now(),
    })
}

/// Recursively expands the subtree rooted at `node_id`.
/// Returns a list of (parent_id, children) pairs for every node that has children
/// in the subtree. Capped at `max_nodes` total nodes (default 50_000) to
/// avoid excessive IPC on very large subtrees.
#[tauri::command]
pub async fn expand_subtree(
    node_id: u32,
    max_nodes: Option<u32>,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<(u32, Vec<NodeDto>)>, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;

    let limit = max_nodes.unwrap_or(50_000) as usize;
    let mut result: Vec<(u32, Vec<NodeDto>)> = Vec::new();
    let mut queue: Vec<u32> = vec![node_id];
    let mut qi = 0;
    let mut total_nodes: usize = 0;

    while qi < queue.len() && total_nodes < limit {
        let parent_id = queue[qi];
        qi += 1;

        let children_ids = index.get_children_slice(parent_id);
        if children_ids.is_empty() {
            continue;
        }

        let remaining_budget = limit.saturating_sub(total_nodes);
        if remaining_budget == 0 {
            break;
        }
        let visible_children_ids = &children_ids[..children_ids
            .len()
            .min(remaining_budget)
            .min(EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT)];
        total_nodes += visible_children_ids.len();

        for &child_id in visible_children_ids {
            if total_nodes < limit && index.has_children(child_id) {
                queue.push(child_id);
            }
        }

        let children: Vec<NodeDto> = visible_children_ids
            .iter()
            .map(|&id| node_to_dto(index, id))
            .collect();
        result.push((parent_id, children));
    }

    Ok(result)
}

#[tauri::command]
pub async fn get_path(
    node_id: u32,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<String, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.get_path(node_id))
}

#[tauri::command]
pub async fn search(
    query: SearchQuery,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<SearchResult>, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let results = index.search(
        &query.text,
        &query.target,
        query.case_sensitive,
        query.regex,
        query.exact_match,
        query.max_results,
        query.path.as_deref(),
        query.multiline,
        query.dot_all,
    );
    let dtos: Vec<SearchResult> = results
        .into_iter()
        .map(|id| {
            let node = &index.nodes[id as usize];
            let value_preview = match node.kind() {
                NodeKind::Str => format!(
                    "\"{}\"",
                    truncate_str(index.str_val_of_node(node), 60)
                ),
                NodeKind::Num => index.number_to_string(id),
                NodeKind::Bool => (node.value_data != 0).to_string(),
                NodeKind::Null => "null".to_string(),
                NodeKind::Object => "[object]".to_string(),
                NodeKind::Array => "[array]".to_string(),
            };
            SearchResult {
                node_id: id,
                file_order: id,
                path: index.get_path(id),
                key: node_key_string(index, id),
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
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<SearchResult>, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
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
                regex_case_insensitive: filter.regex_case_insensitive,
                regex_multiline: filter.regex_multiline,
                regex_dot_all: filter.regex_dot_all,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let match_preview: Arc<str> = Arc::from(build_object_match_preview(&query.filters).as_str());
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
            let children_len = index.children_len(id) as usize;
            // Inline value_preview for objects: avoids building a full NodeDto
            let value_preview = if children_len == 0 {
                "{}".to_string()
            } else {
                format!("{{{} keys}}", children_len)
            };
            SearchResult {
                node_id: id,
                file_order: id,
                path: index.get_path(id),
                key: node_key_string(index, id),
                value_preview,
                kind: "object",
                match_preview: Some(Arc::clone(&match_preview)),
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
    webview_window: tauri::WebviewWindow,
) -> Result<Vec<String>, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.suggest_property_paths(&prefix, limit))
}

/// For each ancestor of node_id (from root to direct parent) returns the pair (id, children),
/// plus the path of the target node, so the frontend can expand the full path
/// and retrieve it in a single IPC call.
#[tauri::command]
pub async fn expand_to(
    node_id: u32,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<ExpandToResult, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;

    let mut chain: Vec<u32> = Vec::new();
    let mut current = node_id;
    loop {
        match index.parent_of(current) {
            None => break,
            Some(p) => {
                chain.push(p);
                current = p;
            }
        }
    }
    chain.reverse(); // from root toward the direct parent

    let expansions: Vec<(u32, Vec<NodeDto>)> = chain
        .into_iter()
        .map(|ancestor_id| {
            let children = index
                .children_iter(ancestor_id)
                .map(|child_id| node_to_dto(index, child_id))
                .collect();
            (ancestor_id, children)
        })
        .collect();

    let path = index.get_path(node_id);

    Ok(ExpandToResult { expansions, path })
}

/// Compact format for get_expanded_slice: tuples instead of JSON objects with repeated
/// field names. Each row is [id, parent_id, key_idx, type, preview, n_children, depth]
/// where key_idx is an index into the local deduplicated key pool (-1 = no key).
/// Typical saving: ~70% vs ExpandedSliceResult with named-field NodeDto objects.
#[derive(Serialize)]
pub struct CompactExpandedSliceResult {
    pub offset: usize,
    pub total_count: usize,
    /// Local pool of unique keys for this slice; indexed by key_idx in the rows.
    pub key_pool: Vec<String>,
    /// [id, parent_id (-1=root), key_idx (-1=none), type_byte, preview, children_count, depth]
    pub rows: Vec<(u32, i32, i32, u8, String, u32, u32)>,
}

#[tauri::command]
pub async fn get_expanded_slice(
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<CompactExpandedSliceResult, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    let slice = index.get_expanded_slice(offset, limit);

    // Local key pool for this slice: avoids repeating the same string
    // for every node that shares a field name (e.g. "id", "name", …).
    let mut key_pool: Vec<String> = Vec::new();
    let mut key_pool_ids: Vec<NodeKey> = Vec::new();

    let mut rows = Vec::with_capacity(slice.len());
    for VisibleSliceRow { id, depth } in slice {
        let node = &index.nodes[id as usize];

        let key_idx: i32 = match node.key() {
            None => -1,
            Some(key) => {
                // linear search on the local pool (usually < 200 entries → cache-friendly)
                match key_pool_ids.iter().position(|&k| k == key) {
                    Some(pos) => pos as i32,
                    None => {
                        let pos = key_pool.len() as i32;
                        key_pool.push(node_key_string(index, id).unwrap_or_default());
                        key_pool_ids.push(key);
                        pos
                    }
                }
            }
        };

        let parent_id_i32 = match index.parent_of(id) {
            None => -1i32,
            Some(p) => p as i32,
        };

        rows.push((
            id,
            parent_id_i32,
            key_idx,
            node_type_byte(node.kind()),
            node_value_preview(index, id).into_owned(),
            index.children_len(id),
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

#[tauri::command]
pub async fn get_raw(
    node_id: u32,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<String, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
    let index = guard.as_ref().ok_or("Nessun file aperto")?;
    Ok(index.build_raw(node_id))
}

/// Opens the subtree of `node_id` in a new window as an independent JSON document.
/// The raw JSON is stored in `pending_content` under the new window's label;
/// the new window reads it via `get_pending_content` on startup.
#[tauri::command]
pub async fn open_in_new_window(
    node_id: u32,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let raw = {
        let idx = state.window_index(webview_window.label());
        let guard = idx.read().unwrap();
        let index = guard.as_ref().ok_or("Nessun file aperto")?;
        index.build_raw(node_id)
    };

    let label = format!(
        "w{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    state
        .pending_content
        .lock()
        .unwrap()
        .insert(label.clone(), raw);

    let new_window =
        tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html".into()))
            .title("JsonGUI")
            .inner_size(1200.0, 800.0)
            .min_inner_size(600.0, 400.0)
            .build()
            .map_err(|e| e.to_string())?;

    // Cleanup on window close
    let app_clone = app.clone();
    let lbl = label.clone();
    new_window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            app_clone.state::<AppState>().remove_window(&lbl);
        }
    });

    Ok(())
}

/// Returns (and consumes) the pre-loaded JSON for this window, if any.
/// Used by windows opened via "Open in new window" to load the subtree.
#[tauri::command]
pub fn get_pending_content(
    webview_window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Option<String> {
    state
        .pending_content
        .lock()
        .unwrap()
        .remove(webview_window.label())
}

#[tauri::command]
pub async fn export_types(
    lang: String,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<String, String> {
    let idx = state.window_index(webview_window.label());
    let guard = idx.read().unwrap();
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
    webview_window: tauri::WebviewWindow,
) -> Result<FileInfo, String> {
    let size_bytes = content.len();
    let index = tauri::async_runtime::spawn_blocking(move || JsonIndex::from_str(&content))
        .await
        .map_err(|e| e.to_string())??;

    let node_count = index.nodes.len();
    let root_node = node_to_dto(&index, index.root);
    let root_children: Vec<NodeDto> = index
        .children_iter(index.root)
        .take(EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT)
        .map(|id| node_to_dto(&index, id))
        .collect();
    *state.window_index(webview_window.label()).write().unwrap() = Some(index);
    Ok(FileInfo {
        node_count,
        size_bytes,
        root_node,
        root_children,
    })
}

#[tauri::command]
pub async fn take_screenshot(
    path: String,
    webview_window: tauri::WebviewWindow,
) -> Result<(), String> {
    use std::process::Command;
    let pos = webview_window.outer_position().map_err(|e| e.to_string())?;
    let size = webview_window.outer_size().map_err(|e| e.to_string())?;
    let scale = webview_window.scale_factor().map_err(|e| e.to_string())?;
    let x = (pos.x as f64 / scale) as i32;
    let y = (pos.y as f64 / scale) as i32;
    let w = (size.width as f64 / scale) as u32;
    let h = (size.height as f64 / scale) as u32;
    let rect = format!("{},{},{},{}", x, y, w, h);
    let status = Command::new("screencapture")
        .args(["-x", "-t", "jpg", "-R", &rect, &path])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("screencapture failed".into())
    }
}

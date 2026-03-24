use crate::json_index::{
    JsonIndex, NodeKey, NodeKind, ObjectSearchFilter as IndexObjectSearchFilter,
    ObjectSearchOperator, SUB_INDEX_ID_RANGE, VisibleSliceRow,
};
// LazyObject and LazyArray are part of NodeKind, accessed via pattern matching
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
    /// Canonical node ID of the target. May differ from the input `node_id` when the
    /// input came from `search_in_lazy_node`, which stores matching nodes in temporary
    /// per-element sub-indices whose IDs conflict with the canonical sub-index created
    /// by `materialize_lazy_node`. `expand_to` resolves the mismatch by re-matching
    /// each chain step via key comparison against the canonical children.
    pub resolved_node_id: u32,
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
        NodeKind::Object | NodeKind::LazyObject => 0,
        NodeKind::Array | NodeKind::LazyArray => 1,
        NodeKind::Str => 2,
        NodeKind::Num => 3,
        NodeKind::Bool => 4,
        NodeKind::Null => 5,
    }
}

fn node_key_string(index: &JsonIndex, node_id: u32) -> Option<String> {
    // For extra nodes (id >= base), delegate to key_string_any
    {
        let extra = index.extra.lock().unwrap();
        if node_id >= extra.base {
            drop(extra);
            return index.key_string_any(node_id);
        }
    }
    match index.nodes[node_id as usize].key()? {
        NodeKey::String(kid) => Some(index.keys.get(kid).to_string()),
        NodeKey::ArrayIndex(idx) => Some(idx.to_string()),
    }
}

/// Text preview of a node's value, shared by node_to_dto and the compact
/// format of get_expanded_slice.
fn node_value_preview(index: &JsonIndex, id: u32) -> Cow<'static, str> {
    // For extra nodes or when id could be out of main nodes range, use value_preview_any
    let is_extra = {
        let extra = index.extra.lock().unwrap();
        id >= extra.base
    };
    if is_extra {
        return Cow::Owned(index.value_preview_any(id));
    }

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
        NodeKind::LazyObject => {
            if index.is_large_lazy(id) {
                Cow::Borrowed("{…}")
            } else if children_len == 0 {
                Cow::Borrowed("{}")
            } else {
                Cow::Owned(format!("{{{} keys}}", children_len))
            }
        }
        NodeKind::LazyArray => {
            if index.is_large_lazy(id) {
                Cow::Borrowed("[…]")
            } else if children_len == 0 {
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
    // For extra nodes, use the any-methods
    let is_extra = {
        let extra = index.extra.lock().unwrap();
        id >= extra.base
    };
    if is_extra {
        return NodeDto {
            id,
            parent_id: index.parent_of_any(id),
            key: node_key_string(index, id),
            value_type: index.value_type_any(id),
            value_preview: Cow::Owned(index.value_preview_any(id)),
            children_count: index.children_count_any(id) as usize,
        };
    }

    let node = &index.nodes[id as usize];
    let children_len = index.children_len(id) as usize;
    let value_type: &'static str = match node.kind() {
        NodeKind::Object => "object",
        NodeKind::Array => "array",
        NodeKind::Str => "string",
        NodeKind::Num => "number",
        NodeKind::Bool => "boolean",
        NodeKind::Null => "null",
        NodeKind::LazyObject => "object",
        NodeKind::LazyArray => "array",
    };
    // For large lazy nodes the exact count is unknown: use u32::MAX as sentinel
    // so the frontend knows to keep loading pages until the server returns an empty page.
    let children_count = if node.kind().is_lazy() && index.is_large_lazy(id) {
        u32::MAX as usize
    } else {
        children_len
    };
    NodeDto {
        id,
        parent_id: index.parent_of(id),
        key: node_key_string(index, id),
        value_type,
        value_preview: node_value_preview(index, id),
        children_count,
    }
}

const PROGRESS_EVENT_THRESHOLD: u64 = 200 * 1024 * 1024; // 200 MB
const EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT: usize = 1_000;

/// How to fetch children for a given node.
enum ChildrenMode {
    /// Regular indexed node — use `children_iter`.
    Normal,
    /// Lazy or extra node with known/small count — use `get_children_any`.
    AnyPath,
    /// Large lazy node — use `get_lazy_children_page` for paginated streaming.
    PagedLazy,
}

/// Classify `node_id` to determine which child-fetching strategy to use.
fn children_mode(index: &JsonIndex, node_id: u32) -> ChildrenMode {
    let extra_base = { index.extra.lock().unwrap().base };
    if node_id >= extra_base {
        return ChildrenMode::AnyPath;
    }
    if !index.nodes[node_id as usize].kind().is_lazy() {
        return ChildrenMode::Normal;
    }
    // is_large_lazy is safe here: node_id < extra_base guarantees a main-index node
    if index.is_large_lazy(node_id) {
        ChildrenMode::PagedLazy
    } else {
        ChildrenMode::AnyPath
    }
}

fn extra_scope_context(
    index: &JsonIndex,
    node_id: u32,
) -> Option<(Arc<JsonIndex>, u32, usize, u32)> {
    let extra = index.extra.lock().unwrap();
    let base = extra.base;
    if node_id < base {
        return None;
    }

    let inner = node_id - base;
    let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
    let sub_id = inner % SUB_INDEX_ID_RANGE;
    let sub_index = Arc::clone(extra.sub_indices.get(sub_idx)?);
    Some((sub_index, sub_id, sub_idx, base))
}

fn map_extra_scope_ids(base: u32, sub_idx: usize, ids: Vec<u32>) -> Vec<u32> {
    ids.into_iter()
        .map(|sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
        .collect()
}

fn is_descendant_of_any(index: &JsonIndex, mut node_id: u32, ancestor_id: u32) -> bool {
    while let Some(parent_id) = index.parent_of_any(node_id) {
        if parent_id == ancestor_id {
            return true;
        }
        node_id = parent_id;
    }
    false
}

fn fetch_children_page_ids(
    index: &JsonIndex,
    node_id: u32,
    offset: usize,
    limit: usize,
) -> Result<Vec<u32>, String> {
    match children_mode(index, node_id) {
        ChildrenMode::Normal => Ok(index
            .children_iter(node_id)
            .skip(offset)
            .take(limit)
            .collect()),
        ChildrenMode::AnyPath => Ok(index
            .get_children_any(node_id)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect()),
        ChildrenMode::PagedLazy => index.get_lazy_children_page(node_id, offset, limit),
    }
}

fn collect_navigation_children_ids(
    index: &JsonIndex,
    parent_id: u32,
    expected_child_key: &str,
) -> Result<Vec<u32>, String> {
    let page_size = EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT;
    let count_hint = index.children_count_any(parent_id) as usize;
    let is_paged_lazy = matches!(children_mode(index, parent_id), ChildrenMode::PagedLazy);
    let use_paging = is_paged_lazy || count_hint > page_size;

    if !use_paging {
        return index.get_children_any(parent_id);
    }

    let target_offset_hint = expected_child_key.parse::<usize>().ok();
    let mut collected: Vec<u32> = Vec::new();
    let mut offset = 0usize;

    loop {
        let page = fetch_children_page_ids(index, parent_id, offset, page_size)?;
        if page.is_empty() {
            break;
        }

        let found = page.iter().any(|&child_id| {
            node_key_string(index, child_id).is_some_and(|k| k == expected_child_key)
        });
        let page_len = page.len();
        collected.extend(page);
        offset += page_len;

        if found {
            break;
        }
        if let Some(target_offset) = target_offset_hint {
            if offset > target_offset {
                break;
            }
        }
        if page_len < page_size {
            break;
        }
        if !is_paged_lazy && count_hint > 0 && offset >= count_hint {
            break;
        }
    }

    Ok(collected)
}

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

    let child_ids = match children_mode(index, node_id) {
        ChildrenMode::Normal => {
            return Ok(index
                .children_iter(node_id)
                .take(EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT)
                .map(|id| node_to_dto(index, id))
                .collect());
        }
        ChildrenMode::AnyPath => index.get_children_any(node_id)?,
        ChildrenMode::PagedLazy => {
            index.get_lazy_children_page(node_id, 0, EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT)?
        }
    };
    Ok(child_ids
        .into_iter()
        .map(|id| node_to_dto(index, id))
        .collect())
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

    let child_ids = match children_mode(index, node_id) {
        ChildrenMode::Normal => {
            return Ok(index
                .children_iter(node_id)
                .skip(offset)
                .take(limit)
                .map(|id| node_to_dto(index, id))
                .collect());
        }
        ChildrenMode::AnyPath => {
            let all = index.get_children_any(node_id)?;
            all.into_iter().skip(offset).take(limit).collect()
        }
        ChildrenMode::PagedLazy => index.get_lazy_children_page(node_id, offset, limit)?,
    };
    Ok(child_ids
        .into_iter()
        .map(|id| node_to_dto(index, id))
        .collect())
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

/// Core BFS shared by `expand_subtree` and `expand_subtree_streaming`.
/// Calls `on_pair(parent_id, children)` for every node that has children.
fn bfs_expand<F>(index: &JsonIndex, node_id: u32, limit: usize, mut on_pair: F)
where
    F: FnMut(u32, Vec<NodeDto>),
{
    let mut queue: Vec<u32> = vec![node_id];
    let mut qi = 0;
    let mut total_nodes: usize = 0;

    while qi < queue.len() && total_nodes < limit {
        let parent_id = queue[qi];
        qi += 1;

        let children_ids = match index.get_children_any(parent_id) {
            Ok(ids) => ids,
            Err(_) => continue,
        };
        if children_ids.is_empty() {
            continue;
        }

        let remaining_budget = limit.saturating_sub(total_nodes);
        if remaining_budget == 0 {
            break;
        }
        let visible_len = children_ids
            .len()
            .min(remaining_budget)
            .min(EXPAND_SUBTREE_MAX_CHILDREN_PER_PARENT);
        total_nodes += visible_len;

        for &child_id in &children_ids[..visible_len] {
            if total_nodes < limit {
                queue.push(child_id);
            }
        }

        let children: Vec<NodeDto> = children_ids[..visible_len]
            .iter()
            .map(|&id| node_to_dto(index, id))
            .collect();
        on_pair(parent_id, children);
    }
}

/// Recursively expands the subtree rooted at `node_id`.
/// Returns a list of (parent_id, children) pairs. Capped at `max_nodes` (default 50_000).
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

    let mut result: Vec<(u32, Vec<NodeDto>)> = Vec::new();
    bfs_expand(
        index,
        node_id,
        max_nodes.unwrap_or(50_000) as usize,
        |parent_id, children| {
            result.push((parent_id, children));
        },
    );
    Ok(result)
}

/// Numero di espansioni (parent, children) per evento "expand-batch".
const EXPAND_STREAMING_BATCH: usize = 200;

/// Variante streaming di `expand_subtree`: emette eventi Tauri "expand-batch" ogni
/// `EXPAND_STREAMING_BATCH` pair e "expand-done" al termine.
#[tauri::command]
pub async fn expand_subtree_streaming(
    node_id: u32,
    max_nodes: Option<u32>,
    state: State<'_, AppState>,
    webview_window: tauri::WebviewWindow,
) -> Result<(), String> {
    let idx = state.window_index(webview_window.label());
    let window = webview_window.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let guard = idx.read().unwrap();
        let index = guard.as_ref().ok_or("Nessun file aperto")?;

        let mut batch: Vec<(u32, Vec<NodeDto>)> = Vec::with_capacity(EXPAND_STREAMING_BATCH);
        bfs_expand(
            index,
            node_id,
            max_nodes.unwrap_or(50_000) as usize,
            |parent_id, children| {
                batch.push((parent_id, children));
                if batch.len() >= EXPAND_STREAMING_BATCH {
                    let _ = window.emit("expand-batch", std::mem::take(&mut batch));
                }
            },
        );

        if !batch.is_empty() {
            let _ = window.emit("expand-batch", batch);
        }
        let _ = window.emit("expand-done", ());
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(())
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
    Ok(index.get_path_any(node_id))
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
    let scope_path = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let scope_node_id = scope_path.and_then(|path| index.resolve_path_any(path));
    if scope_path.is_some() && scope_node_id.is_none() {
        return Ok(Vec::new());
    }

    let results = match scope_node_id {
        Some(scope_id) => {
            if let Some((sub_index, sub_id, sub_idx, base)) = extra_scope_context(index, scope_id) {
                let local_path = sub_index.get_path(sub_id);
                map_extra_scope_ids(
                    base,
                    sub_idx,
                    sub_index.search(
                        &query.text,
                        &query.target,
                        query.case_sensitive,
                        query.regex,
                        query.exact_match,
                        query.max_results,
                        Some(local_path.as_str()),
                        query.multiline,
                        query.dot_all,
                    ),
                )
            } else if index.nodes[scope_id as usize].kind().is_lazy() {
                index.search_in_lazy_node_with_options(
                    scope_id,
                    &query.text,
                    &query.target,
                    query.case_sensitive,
                    query.regex,
                    query.exact_match,
                    query.max_results,
                    query.multiline,
                    query.dot_all,
                )?
            } else {
                let scope_path = index.get_path(scope_id);
                let mut scoped_results = index.search(
                    &query.text,
                    &query.target,
                    query.case_sensitive,
                    query.regex,
                    query.exact_match,
                    query.max_results,
                    Some(scope_path.as_str()),
                    query.multiline,
                    query.dot_all,
                );

                if scoped_results.len() < query.max_results {
                    for (id, node) in index.nodes.iter().enumerate() {
                        if !node.kind().is_lazy() {
                            continue;
                        }
                        let lazy_id = id as u32;
                        if lazy_id != scope_id && !is_descendant_of_any(index, lazy_id, scope_id) {
                            continue;
                        }
                        let remaining = query.max_results.saturating_sub(scoped_results.len());
                        if remaining == 0 {
                            break;
                        }
                        if let Ok(lazy_results) = index.search_in_lazy_node_with_options(
                            lazy_id,
                            &query.text,
                            &query.target,
                            query.case_sensitive,
                            query.regex,
                            query.exact_match,
                            remaining,
                            query.multiline,
                            query.dot_all,
                        ) {
                            scoped_results.extend(lazy_results);
                        }
                        if scoped_results.len() >= query.max_results {
                            break;
                        }
                    }
                }

                scoped_results
            }
        }
        None => {
            let mut unscoped_results = index.search(
                &query.text,
                &query.target,
                query.case_sensitive,
                query.regex,
                query.exact_match,
                query.max_results,
                None,
                query.multiline,
                query.dot_all,
            );

            if unscoped_results.len() < query.max_results {
                for (id, node) in index.nodes.iter().enumerate() {
                    if !node.kind().is_lazy() {
                        continue;
                    }
                    let remaining = query.max_results.saturating_sub(unscoped_results.len());
                    if remaining == 0 {
                        break;
                    }
                    if let Ok(lazy_results) = index.search_in_lazy_node_with_options(
                        id as u32,
                        &query.text,
                        &query.target,
                        query.case_sensitive,
                        query.regex,
                        query.exact_match,
                        remaining,
                        query.multiline,
                        query.dot_all,
                    ) {
                        unscoped_results.extend(lazy_results);
                    }
                    if unscoped_results.len() >= query.max_results {
                        break;
                    }
                }
            }

            unscoped_results
        }
    };

    let base = { index.extra.lock().unwrap().base };
    let dtos: Vec<SearchResult> = results
        .into_iter()
        .map(|id| {
            // For extra nodes (materialized from lazy spans), use the any-methods
            if id >= base {
                let value_preview = index.value_preview_any(id);
                let path = index.get_path_any(id);
                let key = index.key_string_any(id);
                return SearchResult {
                    node_id: id,
                    file_order: id,
                    path,
                    key,
                    value_preview,
                    kind: "node",
                    match_preview: None,
                };
            }
            let node = &index.nodes[id as usize];
            let value_preview = match node.kind() {
                NodeKind::Str => format!("\"{}\"", truncate_str(index.str_val_of_node(node), 60)),
                NodeKind::Num => index.number_to_string(id),
                NodeKind::Bool => (node.value_data != 0).to_string(),
                NodeKind::Null => "null".to_string(),
                NodeKind::Object => "[object]".to_string(),
                NodeKind::Array => "[array]".to_string(),
                NodeKind::LazyObject => "[object]".to_string(),
                NodeKind::LazyArray => "[array]".to_string(),
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
    let scope_path = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let scope_node_id = scope_path.and_then(|path| index.resolve_path_any(path));
    if scope_path.is_some() && scope_node_id.is_none() {
        return Ok(Vec::new());
    }

    let ids = match scope_node_id {
        Some(scope_id) => {
            if let Some((sub_index, sub_id, sub_idx, base)) = extra_scope_context(index, scope_id) {
                let local_path = sub_index.get_path(sub_id);
                map_extra_scope_ids(
                    base,
                    sub_idx,
                    sub_index.search_objects(
                        &filters,
                        query.key_case_sensitive,
                        query.value_case_sensitive,
                        query.max_results,
                        Some(local_path.as_str()),
                    ),
                )
            } else if index.nodes[scope_id as usize].kind().is_lazy() {
                index.search_objects_in_lazy_node(
                    scope_id,
                    &filters,
                    query.key_case_sensitive,
                    query.value_case_sensitive,
                    query.max_results,
                )?
            } else {
                let scope_path = index.get_path(scope_id);
                index.search_objects(
                    &filters,
                    query.key_case_sensitive,
                    query.value_case_sensitive,
                    query.max_results,
                    Some(scope_path.as_str()),
                )
            }
        }
        None => index.search_objects(
            &filters,
            query.key_case_sensitive,
            query.value_case_sensitive,
            query.max_results,
            None,
        ),
    };
    let base = { index.extra.lock().unwrap().base };
    let dtos = ids
        .into_iter()
        .map(|id| {
            let (path, key, value_preview) = if id >= base {
                let count = index.children_count_any(id) as usize;
                let preview = if count == 0 {
                    "{}".to_string()
                } else {
                    format!("{{{} keys}}", count)
                };
                (index.get_path_any(id), index.key_string_any(id), preview)
            } else {
                let count = index.children_len(id) as usize;
                let preview = if count == 0 {
                    "{}".to_string()
                } else {
                    format!("{{{} keys}}", count)
                };
                (index.get_path(id), node_key_string(index, id), preview)
            };
            SearchResult {
                node_id: id,
                file_order: id,
                path,
                key,
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

    let search_path = index.get_path_any(node_id);
    let segments: Vec<&str> = search_path
        .trim()
        .strip_prefix("$.")
        .or_else(|| search_path.trim().strip_prefix('$'))
        .map(|path| {
            path.split('.')
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut expansions: Vec<(u32, Vec<NodeDto>)> = Vec::with_capacity(segments.len());
    let mut current = index.root;
    let mut resolved_node_id = current;

    for segment in segments {
        let children_ids =
            collect_navigation_children_ids(index, current, segment).unwrap_or_default();
        let children: Vec<NodeDto> = children_ids
            .iter()
            .map(|&id| node_to_dto(index, id))
            .collect();
        expansions.push((current, children));

        let Some(next_id) = children_ids
            .iter()
            .find(|&&child_id| node_key_string(index, child_id).is_some_and(|k| k == segment))
            .copied()
        else {
            break;
        };
        current = next_id;
        resolved_node_id = next_id;
    }

    let path = index.get_path_any(resolved_node_id);

    Ok(ExpandToResult {
        expansions,
        path,
        resolved_node_id,
    })
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
    // Uses a HashMap for O(1) dedup — necessary because extra-node IDs can't
    // be compared via NodeKey (which encodes main-index key-pool indices).
    let mut key_pool: Vec<String> = Vec::new();
    let mut key_pool_map: HashMap<String, i32> = HashMap::new();

    let mut rows = Vec::with_capacity(slice.len());
    for VisibleSliceRow { id, depth } in slice {
        let key_idx: i32 = match node_key_string(index, id) {
            None => -1,
            Some(k) => match key_pool_map.get(&k).copied() {
                Some(pos) => pos,
                None => {
                    let pos = key_pool.len() as i32;
                    key_pool_map.insert(k.clone(), pos);
                    key_pool.push(k);
                    pos
                }
            },
        };

        let parent_id_i32 = match index.parent_of_any(id) {
            None => -1i32,
            Some(p) => p as i32,
        };

        let kind = index.node_kind_any(id);
        rows.push((
            id,
            parent_id_i32,
            key_idx,
            node_type_byte(kind),
            node_value_preview(index, id).into_owned(),
            index.children_count_any(id),
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
    Ok(index.get_raw_any(node_id))
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
        index.get_raw_any(node_id)
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

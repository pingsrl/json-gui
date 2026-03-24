use memchr::memmem;
use rayon::prelude::*;
use regex::Regex;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// Containers at depth >= this become lazy nodes (root = depth 0).
pub const LAZY_DEPTH_THRESHOLD: u32 = 1;
/// Lazy nodes with byte span larger than this use paginated scanning instead of full parse.
pub const LAZY_PAGINATE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10 MB
/// Default page size for inline lazy expansion (get_children_any on large lazy nodes).
const EXPAND_SUBTREE_INLINE_PAGE: usize = 1_000;
/// Maximum number of sub-nodes encoded per sub-index in the global ID space.
/// Global ID for extra node: base + sub_idx * SUB_INDEX_ID_RANGE + sub_id.
pub const SUB_INDEX_ID_RANGE: u32 = 1 << 16; // 65536

// ---- InternedStrings ----
// Compact pool for string values: a single Vec<u8> for bytes + open-addressing
// hash table on Vec<u32> (4 bytes/slot). Zero allocations per string,
// zero byte doubling as in HashMap<String> or HashMap<Arc<str>>.
// Memory for N unique strings, T total bytes: T + 13N bytes.

pub struct InternedStrings {
    pub data: Vec<u8>,     // bytes of all unique strings, concatenated
    pub offsets: Vec<u32>, // start of each string in data + final end sentinel
    index: Vec<u32>,       // open-addressing hash table: slot → (id+1), 0=empty
    index_mask: u32,       // capacity - 1 (capacity is a power of 2)
}

impl InternedStrings {
    pub fn new() -> Self {
        Self::with_capacity(0, 0)
    }

    /// Pre-allocates internal buffers to avoid doublings during parsing.
    /// `n_strings`: estimated number of unique strings (for offsets/index).
    /// `data_bytes`: estimated total bytes of unique strings (for data).
    pub fn with_capacity(n_strings: usize, data_bytes: usize) -> Self {
        // The hash table must have power-of-2 capacity ≥ n_strings / 0.75
        let hash_cap = if n_strings == 0 {
            1024usize
        } else {
            let min_cap = (n_strings * 4 / 3).next_power_of_two().max(1024);
            min_cap
        };
        Self {
            data: Vec::with_capacity(data_bytes),
            offsets: {
                let mut offsets = Vec::with_capacity(n_strings + 1);
                offsets.push(0);
                offsets
            },
            index: vec![0u32; hash_cap],
            index_mask: (hash_cap - 1) as u32,
        }
    }

    #[inline]
    fn hash_bytes(bytes: &[u8]) -> u32 {
        #[cfg(target_pointer_width = "64")]
        {
            const ROTATE: u32 = 5;
            const SEED: u64 = 0x517c_c1b7_2722_0a95;

            let mut hash = 0u64;
            let mut chunks = bytes.chunks_exact(8);
            for chunk in &mut chunks {
                let word = u64::from_ne_bytes(chunk.try_into().expect("chunk size is 8"));
                hash = hash.rotate_left(ROTATE) ^ word;
                hash = hash.wrapping_mul(SEED);
            }

            let remainder = chunks.remainder();
            let (head, tail) = remainder.split_at(remainder.len().min(4));
            if head.len() == 4 {
                let word = u32::from_ne_bytes(head.try_into().expect("chunk size is 4")) as u64;
                hash = hash.rotate_left(ROTATE) ^ word;
                hash = hash.wrapping_mul(SEED);
            }
            for &byte in tail {
                hash = hash.rotate_left(ROTATE) ^ byte as u64;
                hash = hash.wrapping_mul(SEED);
            }

            (hash ^ (hash >> 32)) as u32
        }

        #[cfg(target_pointer_width = "32")]
        {
            const ROTATE: u32 = 5;
            const SEED: u32 = 0x2722_0a95;

            let mut hash = 0u32;
            let mut chunks = bytes.chunks_exact(4);
            for chunk in &mut chunks {
                let word = u32::from_ne_bytes(chunk.try_into().expect("chunk size is 4"));
                hash = hash.rotate_left(ROTATE) ^ word;
                hash = hash.wrapping_mul(SEED);
            }

            for &byte in chunks.remainder() {
                hash = hash.rotate_left(ROTATE) ^ byte as u32;
                hash = hash.wrapping_mul(SEED);
            }

            hash
        }
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        // Grow the index if load > 75%
        if (self.len() + 1) * 4 > self.index.len() * 3 {
            self.grow_index();
        }
        let bytes = s.as_bytes();
        let hash = Self::hash_bytes(bytes);
        let mut slot = hash & self.index_mask;
        loop {
            let entry = self.index[slot as usize];
            if entry == 0 {
                // Empty slot: insert new string
                let id = self.len() as u32;
                self.data.extend_from_slice(bytes);
                self.offsets.push(self.data.len() as u32);
                self.index[slot as usize] = id + 1;
                return id;
            }
            // Check if the already-stored string matches
            let eid = (entry - 1) as usize;
            let start = self.offsets[eid] as usize;
            let end = self.offsets[eid + 1] as usize;
            if &self.data[start..end] == bytes {
                return eid as u32;
            }
            // Collision: linear probing
            slot = (slot + 1) & self.index_mask;
        }
    }

    fn grow_index(&mut self) {
        let new_cap = (self.index.len() * 2).max(16);
        let new_mask = (new_cap - 1) as u32;
        let mut new_index = vec![0u32; new_cap];
        for id in 0..self.len() {
            let start = self.offsets[id] as usize;
            let end = self.offsets[id + 1] as usize;
            let hash = Self::hash_bytes(&self.data[start..end]);
            let mut slot = hash & new_mask;
            while new_index[slot as usize] != 0 {
                slot = (slot + 1) & new_mask;
            }
            new_index[slot as usize] = (id as u32) + 1;
        }
        self.index = new_index;
        self.index_mask = new_mask;
    }

    #[inline]
    pub fn get(&self, id: u32) -> &str {
        let start = self.offsets[id as usize] as usize;
        let end = self.offsets[id as usize + 1] as usize;
        // SAFETY: only valid UTF-8 strings inserted via intern(&str)
        unsafe { std::str::from_utf8_unchecked(&self.data[start..end]) }
    }

    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn heap_bytes_estimate(&self) -> usize {
        self.data.capacity()
            + self.offsets.capacity() * std::mem::size_of::<u32>()
            + self.index.capacity() * std::mem::size_of::<u32>()
    }

    /// Releases the hash table used only for interning lookups during parsing.
    /// `get(id)` continues to work; `id_of()` stops returning matches.
    pub fn release_lookup_index(&mut self) {
        self.index = Vec::new();
        self.index_mask = 0;
    }

    /// Looks up the id of an already-interned string without inserting it. O(1) amortized.
    pub fn id_of(&self, s: &str) -> Option<u32> {
        if self.offsets.is_empty() || self.index.is_empty() {
            return None;
        }
        let bytes = s.as_bytes();
        let hash = Self::hash_bytes(bytes);
        let mut slot = hash & self.index_mask;
        loop {
            let entry = self.index[slot as usize];
            if entry == 0 {
                return None; // not found
            }
            let eid = (entry - 1) as usize;
            let start = self.offsets[eid] as usize;
            let end = self.offsets[eid + 1] as usize;
            if &self.data[start..end] == bytes {
                return Some(eid as u32);
            }
            slot = (slot + 1) & self.index_mask;
        }
    }
}

fn shrink_vec_if_wasteful<T>(vec: &mut Vec<T>) {
    // Only shrink if wasted capacity exceeds 25 % of used length (or > 4 MB
    // of raw bytes) — avoids an expensive realloc+copy on large Vecs where
    // the capacity overshoot is just a few elements from the last doubling.
    let waste = vec.capacity().saturating_sub(vec.len());
    let threshold = (vec.len() >> 2).max(1024 * 1024 / std::mem::size_of::<T>().max(1));
    if waste > threshold {
        vec.shrink_to_fit();
    }
}

// ---- Node (8 bytes, 2×u32, no padding) ----
//
// ktype  bits[31:29] = NodeKind (0..5)
// ktype  bits[28:0]  = key data:
//                      - string key id       if bit 28 = 0
//                      - array index         if bit 28 = 1
//                      - NO_KEY sentinel     if all 29 bits = 1
// value_data:
//   - Object/Array → container_meta index
//   - Str          → val_strings id
//   - Num          → inline i31 or nums_pool index
//   - Bool         → 0/1
//   - Null         → 0
//
// Note: the node id (index in the Vec) always coincides with the preorder DFS index,
// because the streaming parser allocates the parent before its children and children in order.

pub const NO_KEY: u32 = 0x1FFF_FFFF; // sentinel: no key (29 bits all ones)
const ARRAY_INDEX_FLAG: u32 = 0x1000_0000;
const KEY_DATA_MASK: u32 = 0x0FFF_FFFF;
const INLINE_NUM_FLAG: u32 = 0x8000_0000;
const INLINE_NUM_MASK: u32 = 0x7FFF_FFFF;
const INLINE_I31_MIN: i64 = -(1 << 30);
const INLINE_I31_MAX: i64 = (1 << 30) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKey {
    String(u32),
    ArrayIndex(u32),
}

/// Node kind packed into the top 3 bits of `ktype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Object = 0,
    Array = 1,
    Str = 2,  // value_data = InternedStrings id in val_strings
    Num = 3,  // value_data = inline i31 or nums_pool index
    Bool = 4, // value_data = 0 (false) or 1 (true)
    Null = 5,
    LazyObject = 6, // value_data = index into lazy_spans; children not parsed yet
    LazyArray = 7,  // value_data = index into lazy_spans; children not parsed yet
}

impl NodeKind {
    #[inline]
    pub fn is_container(self) -> bool {
        // Lazy kinds are NOT containers in the main index sense (0 children there)
        matches!(self, NodeKind::Object | NodeKind::Array)
    }

    #[inline]
    pub fn is_lazy(self) -> bool {
        matches!(self, NodeKind::LazyObject | NodeKind::LazyArray)
    }
}

/// Byte span within the mmap of an unexpanded lazy node.
#[derive(Clone, Copy)]
pub struct LazySpan {
    pub file_offset: u64,
    pub byte_len: u64,
}

/// Materialization info for one lazy node.
pub struct MatInfo2 {
    /// Index into ExtraState::sub_indices.
    pub sub_idx: usize,
    /// Sub-node IDs (within sub_indices[sub_idx]) for the direct children of the lazy node.
    pub child_sub_ids: Vec<u32>,
}

/// Dynamic state accumulated as the user expands lazy nodes on-demand.
pub struct ExtraState {
    /// = main nodes.len() at end of from_file; global IDs for extra nodes start here.
    pub base: u32,
    /// Sub-indices from lazy expansions (one per materialization call).
    pub sub_indices: Vec<Arc<JsonIndex>>,
    /// lazy_node_id → MatInfo2
    pub mat: HashMap<u32, MatInfo2>,
    /// Maps global_extra_id → parent_global_id (the lazy node that was expanded,
    /// or another extra node for deeper nesting).
    pub extra_parent: HashMap<u32, u32>,
    /// Key override for extra nodes whose key can't be stored in the sub-index
    /// (e.g. the element index within a lazy array for search results).
    pub extra_key_override: HashMap<u32, String>,
    /// Cache of already materialized pages for large lazy containers.
    pub page_cache: HashMap<(u32, usize, usize), Vec<u32>>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub ktype: u32,      // bits[31:29]=NodeKind, bits[28:0]=packed NodeKey
    pub value_data: u32, // container meta idx, Str→str_id, Num payload, Bool→0/1
}

impl Node {
    #[inline]
    pub fn kind(&self) -> NodeKind {
        match self.ktype >> 29 {
            0 => NodeKind::Object,
            1 => NodeKind::Array,
            2 => NodeKind::Str,
            3 => NodeKind::Num,
            4 => NodeKind::Bool,
            5 => NodeKind::Null,
            6 => NodeKind::LazyObject,
            7 => NodeKind::LazyArray,
            _ => NodeKind::Null,
        }
    }
    #[inline]
    pub fn key(&self) -> Option<NodeKey> {
        let raw = self.ktype & NO_KEY;
        match raw {
            NO_KEY => None,
            _ if (raw & ARRAY_INDEX_FLAG) != 0 => Some(NodeKey::ArrayIndex(raw & KEY_DATA_MASK)),
            _ => Some(NodeKey::String(raw)),
        }
    }
    #[inline]
    pub fn string_key_id(&self) -> Option<u32> {
        match self.key() {
            Some(NodeKey::String(id)) => Some(id),
            _ => None,
        }
    }
    #[inline]
    pub fn array_index(&self) -> Option<u32> {
        match self.key() {
            Some(NodeKey::ArrayIndex(index)) => Some(index),
            _ => None,
        }
    }
    #[inline]
    pub fn make_ktype(kind: NodeKind, key: Option<NodeKey>) -> u32 {
        let raw = match key {
            None => NO_KEY,
            Some(NodeKey::String(id)) => {
                debug_assert!(id < ARRAY_INDEX_FLAG);
                id
            }
            Some(NodeKey::ArrayIndex(index)) => {
                debug_assert!(index < KEY_DATA_MASK);
                ARRAY_INDEX_FLAG | index
            }
        };
        ((kind as u32) << 29) | raw
    }

    #[inline]
    pub fn is_inline_num(&self) -> bool {
        self.kind() == NodeKind::Num && (self.value_data & INLINE_NUM_FLAG) != 0
    }
}

#[inline]
fn encode_inline_i31(value: i64) -> Option<u32> {
    if !(INLINE_I31_MIN..=INLINE_I31_MAX).contains(&value) {
        return None;
    }
    Some(INLINE_NUM_FLAG | ((value as i32 as u32) & INLINE_NUM_MASK))
}

#[inline]
fn decode_inline_i31(data: u32) -> i32 {
    ((data & INLINE_NUM_MASK) as i32) << 1 >> 1
}

#[inline]
fn subtree_len_from_parts(nodes: &[Node], container_subtrees: &[u32], id: u32) -> u32 {
    let node = &nodes[id as usize];
    if node.kind().is_container() {
        container_subtrees[node.value_data as usize]
    } else {
        0
    }
}

/// Zero-alloc iterator over the direct children of a node.
/// Uses the DFS preorder layout: first child = id+1,
/// next sibling = cur + 1 + nodes[cur].subtree_len.
pub struct ChildrenIter<'a> {
    nodes: &'a [Node],
    container_subtrees: &'a [u32],
    cur: u32,
    remaining: u32,
}

impl<'a> Iterator for ChildrenIter<'a> {
    type Item = u32;
    #[inline]
    fn next(&mut self) -> Option<u32> {
        if self.remaining == 0 {
            return None;
        }
        let id = self.cur;
        self.cur += subtree_len_from_parts(self.nodes, self.container_subtrees, id) + 1;
        self.remaining -= 1;
        Some(id)
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let r = self.remaining as usize;
        (r, Some(r))
    }
}

impl<'a> ExactSizeIterator for ChildrenIter<'a> {}

#[inline]
fn decimal_len_u32(value: u32) -> usize {
    match value {
        0..=9 => 1,
        10..=99 => 2,
        100..=999 => 3,
        1_000..=9_999 => 4,
        10_000..=99_999 => 5,
        100_000..=999_999 => 6,
        1_000_000..=9_999_999 => 7,
        10_000_000..=99_999_999 => 8,
        100_000_000..=999_999_999 => 9,
        _ => 10,
    }
}

#[inline]
fn format_u32_decimal(buf: &mut [u8; 10], mut value: u32) -> &str {
    let mut cursor = buf.len();
    loop {
        cursor -= 1;
        buf[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    // SAFETY: the buffer only contains ASCII decimal digits.
    unsafe { std::str::from_utf8_unchecked(&buf[cursor..]) }
}

struct StackString<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackString<N> {
    #[inline]
    fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    fn as_str(&self) -> &str {
        // SAFETY: only valid UTF-8 is written via fmt::Write.
        unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
}

impl<const N: usize> std::fmt::Write for StackString<N> {
    #[inline]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let end = self.len.checked_add(bytes.len()).ok_or(std::fmt::Error)?;
        if end > N {
            return Err(std::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}

#[inline]
fn format_f64_display<'a, const N: usize>(buf: &'a mut StackString<N>, value: f64) -> &'a str {
    buf.clear();
    write!(buf, "{value}").expect("stack buffer too small for f64 formatting");
    buf.as_str()
}

#[inline]
fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

#[inline]
fn starts_with_case_insensitive(candidate: &str, prefix: &str, prefix_lower: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if candidate.starts_with(prefix) {
        return true;
    }
    if candidate.is_ascii() && prefix.is_ascii() {
        candidate
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    } else {
        candidate.to_lowercase().starts_with(prefix_lower)
    }
}

#[inline]
fn matches_text(
    haystack: &str,
    query: &str,
    query_lower: &str,
    case_sensitive: bool,
    exact_match: bool,
) -> bool {
    if case_sensitive {
        if exact_match {
            haystack == query
        } else {
            haystack.contains(query)
        }
    } else if haystack.is_ascii() && query.is_ascii() {
        if exact_match {
            haystack.eq_ignore_ascii_case(query)
        } else {
            contains_ascii_case_insensitive(haystack, query)
        }
    } else {
        let haystack_lower = haystack.to_lowercase();
        if exact_match {
            haystack_lower == query_lower
        } else {
            haystack_lower.contains(query_lower)
        }
    }
}

// ---- Streaming parser: zero intermediate allocations ----
//
// Final Nodes are allocated DIRECTLY during DFS streaming.
// subtree_len is set after all children are processed (no finish_index heavy pass).

struct StreamCtx {
    nodes: Vec<Node>,
    /// Parent distance packed as u8.  u8::MAX (255) = overflow → see parent_overflow_*.
    parent_deltas: Vec<u8>,
    parent_overflow_ids: Vec<u32>,
    parent_overflow_values: Vec<u32>,
    container_subtrees: Vec<u32>,
    container_children: Vec<u32>,
    keys: InternedStrings,
    val_strings: InternedStrings,
    nums_pool: Vec<f64>, // f64 pool: NodeKind::Num → nums_pool[value_data]
    /// Maximum bytes to store per string value (0 = unlimited).
    str_max_bytes: usize,
    // ---- Lazy mode fields (active when lazy_depth > 0) ----
    lazy_spans: Vec<LazySpan>,
    lazy_child_counts: Vec<u16>,
    /// Base pointer of the mmap as usize (0 if lazy mode disabled).
    mmap_base: usize,
    /// Depth threshold: containers at depth >= lazy_depth become lazy nodes (0 = disabled).
    lazy_depth: u32,
}

impl StreamCtx {
    /// Pre-allocates the main Vecs to avoid doublings during parsing.
    /// `node_cap`    = estimated nodes (file_size / 50).
    /// `str_bytes`   = estimated unique bytes for val_strings (file_size / 10).
    fn with_capacity(node_cap: usize, str_bytes: usize) -> Self {
        // JSON keys: few short strings (e.g. field names). Estimate: 1% of nodes, 20 bytes/key.
        let key_n = (node_cap / 100).max(64);
        let key_bytes = key_n * 20;
        let parent_overflow_cap = (node_cap / 256).max(16);
        let container_cap = (node_cap / 4).max(64);
        // String values: ~30% of nodes, deduplicated bytes already passed as str_bytes.
        let val_n = node_cap * 3 / 10;
        // Numbers: ~20% of nodes.
        let num_cap = node_cap / 5;
        Self {
            nodes: Vec::with_capacity(node_cap),
            parent_deltas: Vec::with_capacity(node_cap),
            parent_overflow_ids: Vec::with_capacity(parent_overflow_cap),
            parent_overflow_values: Vec::with_capacity(parent_overflow_cap),
            container_subtrees: Vec::with_capacity(container_cap),
            container_children: Vec::with_capacity(container_cap),
            keys: InternedStrings::with_capacity(key_n, key_bytes),
            val_strings: InternedStrings::with_capacity(val_n, str_bytes),
            nums_pool: Vec::with_capacity(num_cap),
            str_max_bytes: 0,
            lazy_spans: Vec::new(),
            lazy_child_counts: Vec::new(),
            mmap_base: 0,
            lazy_depth: 0,
        }
    }

    fn alloc(&mut self, kind: NodeKind, key: Option<NodeKey>, value_data: u32, parent: u32) -> u32 {
        let id = self.nodes.len() as u32;
        let value_data = if kind.is_container() {
            let meta_id = self.container_subtrees.len() as u32;
            self.container_subtrees.push(0);
            self.container_children.push(0);
            meta_id
        } else {
            value_data
        };
        self.nodes.push(Node {
            ktype: Node::make_ktype(kind, key),
            value_data,
        });
        if parent == u32::MAX {
            self.parent_deltas.push(0);
        } else {
            let delta = id - parent;
            if delta < u8::MAX as u32 {
                self.parent_deltas.push(delta as u8);
            } else {
                self.parent_deltas.push(u8::MAX);
                self.parent_overflow_ids.push(id);
                self.parent_overflow_values.push(parent);
            }
        }
        if parent != u32::MAX {
            let parent_node = &self.nodes[parent as usize];
            debug_assert!(parent_node.kind().is_container());
            let children = &mut self.container_children[parent_node.value_data as usize];
            *children = children.saturating_add(1);
        }
        id
    }

    /// Allocates a lazy container node (LazyObject or LazyArray).
    /// `span_id` = index into lazy_spans for this node's raw bytes.
    /// The parent MUST be a normal container (Object/Array) already allocated.
    fn alloc_lazy(
        &mut self,
        kind: NodeKind,
        key: Option<NodeKey>,
        span_id: u32,
        parent: u32,
    ) -> u32 {
        debug_assert!(kind.is_lazy());
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            ktype: Node::make_ktype(kind, key),
            value_data: span_id,
        });
        let delta = id - parent;
        if delta < u8::MAX as u32 {
            self.parent_deltas.push(delta as u8);
        } else {
            self.parent_deltas.push(u8::MAX);
            self.parent_overflow_ids.push(id);
            self.parent_overflow_values.push(parent);
        }
        // Update parent's children count
        let parent_node = &self.nodes[parent as usize];
        debug_assert!(parent_node.kind().is_container());
        let children = &mut self.container_children[parent_node.value_data as usize];
        *children = children.saturating_add(1);
        // Lazy node contributes 0 to parent's subtree_len for DFS traversal purposes —
        // it is treated as a leaf in the main index (subtree_len = 0).
        id
    }
}

#[derive(Clone, Copy)]
struct StreamCtxPtr<'a> {
    ptr: NonNull<StreamCtx>,
    _marker: PhantomData<&'a mut StreamCtx>,
}

impl<'a> StreamCtxPtr<'a> {
    fn new(ctx: &'a mut StreamCtx) -> Self {
        Self {
            ptr: NonNull::from(ctx),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn with_mut<R>(self, f: impl FnOnce(&mut StreamCtx) -> R) -> R {
        // SAFETY: parsing is strictly single-threaded and synchronous; the context
        // outlives all visitors/seeds created during parse_streaming_with_cap.
        unsafe {
            f(self
                .ptr
                .as_ptr()
                .as_mut()
                .expect("stream ctx pointer is null"))
        }
    }
}

struct ValSeed {
    ctx: StreamCtxPtr<'static>,
    parent: u32,
    key: Option<NodeKey>,
    depth: u32,
}

impl<'de> DeserializeSeed<'de> for ValSeed {
    type Value = u32;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<u32, D::Error> {
        de.deserialize_any(ValVisitor {
            ctx: self.ctx,
            parent: self.parent,
            key: self.key,
            depth: self.depth,
        })
    }
}

struct ValVisitor {
    ctx: StreamCtxPtr<'static>,
    parent: u32,
    key: Option<NodeKey>,
    depth: u32,
}

impl<'de> Visitor<'de> for ValVisitor {
    type Value = u32;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a JSON value")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<u32, E> {
        Ok(self
            .ctx
            .with_mut(|ctx| ctx.alloc(NodeKind::Bool, self.key, v as u32, self.parent)))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
        Ok(self.ctx.with_mut(|ctx| {
            let value_data = match encode_inline_i31(v) {
                Some(inline) => inline,
                None => {
                    let nid = ctx.nums_pool.len() as u32;
                    ctx.nums_pool.push(v as f64);
                    nid
                }
            };
            ctx.alloc(NodeKind::Num, self.key, value_data, self.parent)
        }))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
        Ok(self.ctx.with_mut(|ctx| {
            let value_data = if v <= INLINE_I31_MAX as u64 {
                encode_inline_i31(v as i64).expect("u64 range pre-checked")
            } else {
                let nid = ctx.nums_pool.len() as u32;
                ctx.nums_pool.push(v as f64);
                nid
            };
            ctx.alloc(NodeKind::Num, self.key, value_data, self.parent)
        }))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<u32, E> {
        Ok(self.ctx.with_mut(|ctx| {
            let value_data = if v.fract() == 0.0 {
                encode_inline_i31(v as i64)
                    .filter(|_| v >= INLINE_I31_MIN as f64 && v <= INLINE_I31_MAX as f64)
            } else {
                None
            }
            .unwrap_or_else(|| {
                let nid = ctx.nums_pool.len() as u32;
                ctx.nums_pool.push(v);
                nid
            });
            ctx.alloc(NodeKind::Num, self.key, value_data, self.parent)
        }))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
        Ok(self.ctx.with_mut(|ctx| {
            let s = if ctx.str_max_bytes > 0 && v.len() > ctx.str_max_bytes {
                // Truncate at a valid UTF-8 boundary to cap memory usage.
                let mut end = ctx.str_max_bytes;
                while !v.is_char_boundary(end) {
                    end -= 1;
                }
                &v[..end]
            } else {
                v
            };
            let sid = ctx.val_strings.intern(s);
            ctx.alloc(NodeKind::Str, self.key, sid, self.parent)
        }))
    }
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<u32, E> {
        // sonic-rs provides borrowed slices for escape-free strings; we use the
        // bytes directly for hash/comparison without an extra copy, then intern
        // them (copy only if new) into the compact val_strings pool.
        self.visit_str(v)
    }
    fn visit_unit<E: de::Error>(self) -> Result<u32, E> {
        Ok(self
            .ctx
            .with_mut(|ctx| ctx.alloc(NodeKind::Null, self.key, 0, self.parent)))
    }
    fn visit_none<E: de::Error>(self) -> Result<u32, E> {
        Ok(self
            .ctx
            .with_mut(|ctx| ctx.alloc(NodeKind::Null, self.key, 0, self.parent)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<u32, A::Error> {
        let id = self
            .ctx
            .with_mut(|ctx| ctx.alloc(NodeKind::Object, self.key, 0, self.parent));
        let use_lazy = self
            .ctx
            .with_mut(|ctx| ctx.lazy_depth > 0 && self.depth + 1 >= ctx.lazy_depth);
        while let Some(key_str) = map.next_key::<Cow<'de, str>>()? {
            let kid = self.ctx.with_mut(|ctx| ctx.keys.intern(&key_str));
            if use_lazy {
                // Consume value as raw LazyValue
                let raw_val = map
                    .next_value::<sonic_rs::LazyValue>()
                    .map_err(|e| de::Error::custom(e.to_string()))?;
                let raw_bytes = raw_val.as_raw_str().as_bytes();
                match raw_bytes.first().copied() {
                    Some(b'{') | Some(b'[') => {
                        let is_obj = raw_bytes[0] == b'{';
                        let kind = if is_obj {
                            NodeKind::LazyObject
                        } else {
                            NodeKind::LazyArray
                        };
                        let placeholder_count = estimate_children_count(raw_bytes);
                        self.ctx.with_mut(|ctx| {
                            let mmap_base = ctx.mmap_base;
                            let file_offset = raw_bytes.as_ptr() as u64 - mmap_base as u64;
                            let byte_len = raw_bytes.len() as u64;
                            let span_id = ctx.lazy_spans.len() as u32;
                            ctx.lazy_spans.push(LazySpan {
                                file_offset,
                                byte_len,
                            });
                            ctx.lazy_child_counts.push(placeholder_count);
                            ctx.alloc_lazy(kind, Some(NodeKey::String(kid)), span_id, id);
                        });
                    }
                    _ => {
                        // Scalar at lazy depth: re-parse from raw bytes via ValSeed
                        let mut sub_de = sonic_rs::Deserializer::from_slice(raw_bytes);
                        ValSeed {
                            ctx: self.ctx,
                            parent: id,
                            key: Some(NodeKey::String(kid)),
                            depth: self.depth + 1,
                        }
                        .deserialize(&mut sub_de)
                        .map_err(|e| de::Error::custom(e.to_string()))?;
                    }
                }
                // No subtree_len update needed for lazy children (they count as 0-len)
                // But we need to update the parent's subtree contribution for this child.
                // Lazy nodes are leaves in main index: subtree_len contribution = 1 (just themselves).
                // This is handled below after the loop via nodes.len() - id - 1.
            } else {
                map.next_value_seed(ValSeed {
                    ctx: self.ctx,
                    parent: id,
                    key: Some(NodeKey::String(kid)),
                    depth: self.depth + 1,
                })?;
            }
        }
        // Set subtree_len AFTER all children are allocated
        self.ctx.with_mut(|ctx| {
            let meta_id = ctx.nodes[id as usize].value_data as usize;
            ctx.container_subtrees[meta_id] = ctx.nodes.len() as u32 - id - 1;
        });
        Ok(id)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u32, A::Error> {
        let id = self
            .ctx
            .with_mut(|ctx| ctx.alloc(NodeKind::Array, self.key, 0, self.parent));
        let use_lazy = self
            .ctx
            .with_mut(|ctx| ctx.lazy_depth > 0 && self.depth + 1 >= ctx.lazy_depth);
        let mut index = 0usize;
        loop {
            if index >= KEY_DATA_MASK as usize {
                return Err(de::Error::custom(
                    "array index exceeds inline storage capacity",
                ));
            }
            if use_lazy {
                let raw_opt = seq
                    .next_element::<sonic_rs::LazyValue>()
                    .map_err(|e| de::Error::custom(e.to_string()))?;
                let Some(lazy) = raw_opt else { break };
                let raw_bytes = lazy.as_raw_str().as_bytes();
                match raw_bytes.first().copied() {
                    Some(b'{') | Some(b'[') => {
                        let is_obj = raw_bytes[0] == b'{';
                        let kind = if is_obj {
                            NodeKind::LazyObject
                        } else {
                            NodeKind::LazyArray
                        };
                        let placeholder_count = estimate_children_count(raw_bytes);
                        self.ctx.with_mut(|ctx| {
                            let mmap_base = ctx.mmap_base;
                            let file_offset = raw_bytes.as_ptr() as u64 - mmap_base as u64;
                            let byte_len = raw_bytes.len() as u64;
                            let span_id = ctx.lazy_spans.len() as u32;
                            ctx.lazy_spans.push(LazySpan {
                                file_offset,
                                byte_len,
                            });
                            ctx.lazy_child_counts.push(placeholder_count);
                            ctx.alloc_lazy(
                                kind,
                                Some(NodeKey::ArrayIndex(index as u32)),
                                span_id,
                                id,
                            );
                        });
                    }
                    _ => {
                        // Scalar: re-parse from raw bytes via ValSeed
                        let mut sub_de = sonic_rs::Deserializer::from_slice(raw_bytes);
                        ValSeed {
                            ctx: self.ctx,
                            parent: id,
                            key: Some(NodeKey::ArrayIndex(index as u32)),
                            depth: self.depth + 1,
                        }
                        .deserialize(&mut sub_de)
                        .map_err(|e| de::Error::custom(e.to_string()))?;
                    }
                }
            } else {
                if seq
                    .next_element_seed(ValSeed {
                        ctx: self.ctx,
                        parent: id,
                        key: Some(NodeKey::ArrayIndex(index as u32)),
                        depth: self.depth + 1,
                    })?
                    .is_none()
                {
                    break;
                }
            }
            index += 1;
        }
        // Set subtree_len AFTER all children are allocated
        self.ctx.with_mut(|ctx| {
            let meta_id = ctx.nodes[id as usize].value_data as usize;
            ctx.container_subtrees[meta_id] = ctx.nodes.len() as u32 - id - 1;
        });
        Ok(id)
    }
}

/// Quick estimate of the number of direct children in a JSON container byte slice.
/// Returns min(count, 65535). Returns 0 for empty containers.
/// Returns 0 if the container is empty, 1 if it has any children.
/// O(n) worst-case but stops at the first non-whitespace byte after the opening bracket,
/// so in practice it's O(1) for non-empty containers (the common case).
/// We deliberately avoid a full comma-count scan (which would double the I/O cost
/// for large files) — the exact count is revealed when the user expands the node.
fn estimate_children_count(bytes: &[u8]) -> u16 {
    if bytes.len() <= 2 {
        return 0;
    }
    // Look for any non-whitespace byte between the outer brackets.
    for &b in &bytes[1..bytes.len() - 1] {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => continue,
            _ => return 1,
        }
    }
    0
}

// ── Custom byte-level JSON scanners ──────────────────────────────────────────

/// One entry at the top level of a JSON object or array.
struct TopLevelEntry {
    key: String, // empty for array roots
    value_start: usize,
    value_end: usize,
    is_container: bool,
    is_array: bool,
}

/// Scan to end of a JSON string literal starting at `bytes[start]` (must be `"`).
/// Returns the index one past the closing `"`.
#[inline]
pub fn scan_json_string(bytes: &[u8], start: usize) -> usize {
    let mut pos = start + 1; // skip opening "
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' => {
                pos = (pos + 2).min(bytes.len());
            } // skip escaped char (clamp to avoid OOB)
            b'"' => {
                pos += 1;
                break;
            }
            _ => {
                pos += 1;
            }
        }
    }
    pos
}

/// Scan to the end of a JSON value starting at `bytes[start]`.
/// Returns the index one past the last byte of the value.
pub fn scan_json_value_end(bytes: &[u8], start: usize) -> usize {
    let mut pos = start;
    if pos >= bytes.len() {
        return pos;
    }
    match bytes[pos] {
        b'{' | b'[' => {
            let open = bytes[pos];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 1i32;
            pos += 1;
            while pos < bytes.len() {
                match bytes[pos] {
                    b'"' => {
                        pos = scan_json_string(bytes, pos);
                    }
                    b if b == open => {
                        depth += 1;
                        pos += 1;
                    }
                    b if b == close => {
                        depth -= 1;
                        pos += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {
                        pos += 1;
                    }
                }
            }
        }
        b'"' => {
            pos = scan_json_string(bytes, pos);
        }
        _ => {
            // null, true, false, or number
            while pos < bytes.len()
                && !matches!(
                    bytes[pos],
                    b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}'
                )
            {
                pos += 1;
            }
        }
    }
    pos
}

/// Scan array or object `bytes` to find element byte ranges.
/// Returns `(elem_start, elem_end)` pairs (exclusive end), skipping first `offset` elements.
pub fn scan_json_elements(
    bytes: &[u8],
    is_array: bool,
    offset: usize,
    limit: usize,
) -> Vec<(usize, usize)> {
    let mut pos = 0usize;
    // Skip to opening '[' or '{'
    while pos < bytes.len() && !matches!(bytes[pos], b'[' | b'{') {
        pos += 1;
    }
    if pos >= bytes.len() {
        return Vec::new();
    }
    pos += 1; // skip the opening bracket

    let mut results = Vec::new();
    let mut skipped = 0usize;

    loop {
        // Skip whitespace and commas
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b',') {
            pos += 1;
        }
        if pos >= bytes.len() || matches!(bytes[pos], b']' | b'}') {
            break;
        }

        let elem_start = pos;

        if !is_array {
            // Object: skip key
            if pos < bytes.len() && bytes[pos] == b'"' {
                pos = scan_json_string(bytes, pos);
                // Skip whitespace and colon
                while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b':')
                {
                    pos += 1;
                }
            }
        }

        // Scan value
        pos = scan_json_value_end(bytes, pos);
        let elem_end = pos;

        if skipped < offset {
            skipped += 1;
            continue;
        }

        results.push((elem_start, elem_end));
        if results.len() >= limit {
            break;
        }
    }

    results
}

/// Use reverse scan from EOF to find the end position of a container value
/// that starts at `value_start` with `open_char` (`[` or `{`).
/// Returns one-past-the-closing-bracket position.
fn find_value_end_reverse(bytes: &[u8], open_char: u8, value_start: usize) -> usize {
    let close_char = if open_char == b'[' { b']' } else { b'}' };
    let mut pos = bytes.len();

    // We need to scan backwards, skipping strings carefully.
    // Strategy: collect positions of all non-string close_chars from the end.
    // We track depth by counting open/close brackets (not inside strings).
    // Since we're going backwards, we start depth=0 and look for the FIRST close we hit
    // (which is the outermost closing bracket from the end).

    // First skip trailing whitespace from end
    while pos > value_start && matches!(bytes[pos - 1], b' ' | b'\t' | b'\n' | b'\r') {
        pos -= 1;
    }

    // The file structure is: ..., "value": <our_value>,...closing_of_root}
    // We need to find the closing bracket of <our_value>.
    // Method: scan backwards from end, counting bracket depth.
    // The root's closing bracket has depth 1 (from root open), and our value's closing
    // bracket has depth 2 from the end perspective.
    // Simpler: just count from value_start forward for LARGE values is too slow.
    // We do reverse scan but must handle strings correctly.

    // Build a simple forward index of string ranges in the last ~64KB to handle
    // string boundaries correctly in reverse. For values that end near EOF this is fine.
    // If value is the last thing in the file, last char before root-close is close_char.

    // Simple approach: scan backwards from end, tracking depth.
    // Strings in reverse: find `"` then scan forward to verify it's a string end.
    // We approximate by just counting brackets (ignoring strings in reverse).
    // This works for typical JSON where brackets inside strings are rare.

    let mut depth = 0i32;
    let mut p = pos;
    while p > value_start {
        p -= 1;
        let b = bytes[p];
        if b == b'"' {
            // Skip backwards over a string literal
            // Find the opening quote by going forward from value_start is too expensive.
            // Instead: scan backwards counting escape sequences.
            // The byte before the `"` could be `\` (escaped quote) or not.
            let mut q = p;
            loop {
                if q == 0 {
                    break;
                }
                q -= 1;
                if bytes[q] != b'\\' {
                    break;
                }
                // Count consecutive backslashes
                let mut bs = 0usize;
                let mut bq = q;
                while bq > 0 && bytes[bq] == b'\\' {
                    bs += 1;
                    bq -= 1;
                }
                if bs % 2 == 1 {
                    // odd backslashes: this `"` is escaped, keep scanning
                    p = q;
                    continue;
                }
                break;
            }
            // q is at the opening `"` of the string (approximately)
            continue;
        }
        if b == close_char {
            depth -= 1;
            if depth == -1 {
                // This is the matching close for our value
                return p + 1;
            }
        } else if b == open_char {
            depth += 1;
        }
    }

    bytes.len() // fallback: return entire remainder
}

/// Threshold for "small value" forward scan budget per entry.
const FAST_SCAN_SMALL_VALUE_THRESHOLD: usize = 2 * 1024 * 1024; // 2 MB

/// Attempt to scan the end of a JSON value starting at `bytes[start]`,
/// but stop after at most `budget` bytes of forward scanning.
/// Returns `Some(end_pos)` if the value ends within budget, `None` if the budget is exceeded.
fn scan_json_value_end_budgeted(bytes: &[u8], start: usize, budget: usize) -> Option<usize> {
    let limit = (start + budget).min(bytes.len());
    let slice = &bytes[start..limit];
    if slice.is_empty() {
        return Some(start);
    }
    match slice[0] {
        b'{' | b'[' => {
            let open = slice[0];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 1i32;
            let mut rel = 1usize; // relative position within slice
            while rel < slice.len() {
                match slice[rel] {
                    b'"' => {
                        // scan string using the slice
                        rel = scan_json_string(slice, rel);
                    }
                    b if b == open => {
                        depth += 1;
                        rel += 1;
                    }
                    b if b == close => {
                        depth -= 1;
                        rel += 1;
                        if depth == 0 {
                            return Some(start + rel);
                        }
                    }
                    _ => {
                        rel += 1;
                    }
                }
            }
            None // hit budget limit without finding close
        }
        b'"' => {
            let end_rel = scan_json_string(slice, 0);
            if end_rel <= slice.len() {
                Some(start + end_rel)
            } else {
                None
            }
        }
        _ => {
            // scalar
            let mut pos = 0usize;
            while pos < slice.len()
                && !matches!(
                    slice[pos],
                    b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}'
                )
            {
                pos += 1;
            }
            if pos < slice.len() || start + pos == bytes.len() {
                Some(start + pos)
            } else {
                None
            }
        }
    }
}

/// Scan top-level entries of a JSON root object or array.
/// For small values (< FAST_SCAN_SMALL_VALUE_THRESHOLD) uses forward scan.
/// For large values uses reverse scan from EOF.
fn scan_top_level_entries(bytes: &[u8]) -> Result<Vec<TopLevelEntry>, String> {
    let mut pos = 0usize;

    // Find root bracket
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    if pos >= bytes.len() {
        return Err("empty JSON".to_string());
    }

    let root_char = bytes[pos];
    if root_char != b'{' && root_char != b'[' {
        return Err(format!(
            "expected '{{' or '[' at root, got '{}'",
            root_char as char
        ));
    }
    let is_array_root = root_char == b'[';
    pos += 1;

    // Find root close position from the end (for reverse scan fallback)
    // The root's closing bracket is the last non-whitespace byte of the file.
    let mut root_close_pos = bytes.len();
    while root_close_pos > 0 && matches!(bytes[root_close_pos - 1], b' ' | b'\t' | b'\n' | b'\r') {
        root_close_pos -= 1;
    }
    // root_close_pos now points one past the root's closing bracket

    let mut entries: Vec<TopLevelEntry> = Vec::new();
    let mut array_idx = 0usize;

    loop {
        // Skip whitespace/commas
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b',') {
            pos += 1;
        }
        if pos >= bytes.len() || matches!(bytes[pos], b']' | b'}') {
            break;
        }

        let key = if is_array_root {
            let k = array_idx.to_string();
            array_idx += 1;
            k
        } else {
            // Read key string
            if bytes[pos] != b'"' {
                return Err(format!("expected '\"' for key at pos {pos}"));
            }
            let key_start = pos + 1;
            pos = scan_json_string(bytes, pos);
            let key_end = pos - 1;
            let key_raw = bytes.get(key_start..key_end).unwrap_or(b"");
            String::from_utf8_lossy(key_raw).into_owned()
        };

        if !is_array_root {
            // Skip whitespace and colon
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b':') {
                pos += 1;
            }
        }

        // Record value start
        let value_start = pos;
        if pos >= bytes.len() {
            break;
        }

        let first_byte = bytes[pos];
        let is_container = matches!(first_byte, b'{' | b'[');
        let is_array_val = first_byte == b'[';

        // Try fast forward scan with budget; fall back to full scan for large values.
        // We must NOT break here even for large values — there may be more entries after this one.
        let value_end =
            match scan_json_value_end_budgeted(bytes, value_start, FAST_SCAN_SMALL_VALUE_THRESHOLD)
            {
                Some(end) => end,
                None => {
                    // Value exceeds quick-scan budget — do a full (unbounded) forward scan.
                    let remaining = bytes.len().saturating_sub(value_start);
                    scan_json_value_end_budgeted(bytes, value_start, remaining)
                        .unwrap_or_else(|| find_value_end_reverse(bytes, first_byte, value_start))
                }
            };

        pos = value_end;

        entries.push(TopLevelEntry {
            key,
            value_start,
            value_end,
            is_container,
            is_array: is_array_val,
        });
    }

    Ok(entries)
}

/// Finalizes the JsonIndex: trivial finish since subtree_len is already set during streaming.
/// Leaf nodes do not allocate container metadata and implicitly have subtree_len=0.
fn finish_index(ctx: StreamCtx) -> JsonIndex {
    let StreamCtx {
        mut nodes,
        mut parent_deltas,
        mut parent_overflow_ids,
        mut parent_overflow_values,
        mut container_subtrees,
        mut container_children,
        keys: mut keys_pool,
        mut val_strings,
        mut nums_pool,
        str_max_bytes: _,
        lazy_spans,
        lazy_child_counts,
        mmap_base: _,
        lazy_depth: _,
    } = ctx;

    // Shrink all Vecs to exact length, releasing over-allocated capacity.
    shrink_vec_if_wasteful(&mut nodes);
    shrink_vec_if_wasteful(&mut parent_deltas);
    shrink_vec_if_wasteful(&mut parent_overflow_ids);
    shrink_vec_if_wasteful(&mut parent_overflow_values);
    shrink_vec_if_wasteful(&mut nums_pool);

    // Release the value-strings hash table – only needed during parsing.
    // Keep keys_pool.index: it is used by search (keys.id_of fast-path).
    val_strings.release_lookup_index();
    shrink_vec_if_wasteful(&mut val_strings.data);
    shrink_vec_if_wasteful(&mut val_strings.offsets);
    shrink_vec_if_wasteful(&mut keys_pool.data);
    shrink_vec_if_wasteful(&mut keys_pool.offsets);

    shrink_vec_if_wasteful(&mut container_subtrees);
    shrink_vec_if_wasteful(&mut container_children);

    let base = nodes.len() as u32;

    JsonIndex {
        nodes,
        parent_deltas,
        parent_overflow_ids,
        parent_overflow_values,
        container_subtrees,
        container_children,
        keys: keys_pool,
        val_strings,
        nums_pool,
        root: 0,
        lazy_spans,
        lazy_child_counts,
        source_file: None, // set by from_file after parsing for large files
        source_mmap: None,
        extra: Mutex::new(ExtraState {
            base,
            sub_indices: Vec::new(),
            mat: HashMap::new(),
            extra_parent: HashMap::new(),
            extra_key_override: HashMap::new(),
            page_cache: HashMap::new(),
        }),
    }
}

fn parse_streaming<'de, D: de::Deserializer<'de>>(de: D) -> Result<JsonIndex, D::Error> {
    parse_streaming_with_cap(de, 0, 0, 0, 0, 0)
}

fn parse_streaming_with_cap<'de, D: de::Deserializer<'de>>(
    de: D,
    node_cap: usize,
    str_bytes: usize,
    str_max_bytes: usize,
    lazy_depth: u32,
    mmap_base: usize,
) -> Result<JsonIndex, D::Error> {
    let mut ctx = StreamCtx::with_capacity(node_cap, str_bytes);
    ctx.str_max_bytes = str_max_bytes;
    ctx.lazy_depth = lazy_depth;
    ctx.mmap_base = mmap_base;
    let ctx_ptr = StreamCtxPtr::new(&mut ctx);
    let ctx_ptr: StreamCtxPtr<'static> = unsafe { std::mem::transmute(ctx_ptr) };
    de.deserialize_any(ValVisitor {
        ctx: ctx_ptr,
        parent: u32::MAX,
        key: None,
        depth: 0,
    })?;
    Ok(finish_index(ctx))
}

// ---- JsonIndex ----

pub struct JsonIndex {
    pub nodes: Vec<Node>,
    /// Parent distance packed as u8 (255 = overflow → parent_overflow_*).
    pub parent_deltas: Vec<u8>,
    pub parent_overflow_ids: Vec<u32>,
    pub parent_overflow_values: Vec<u32>,
    pub container_subtrees: Vec<u32>, // subtree_len per container (same index as container_meta)
    pub container_children: Vec<u32>, // children_len per container
    pub keys: InternedStrings,
    pub val_strings: InternedStrings, // compact interned string-value pool
    pub nums_pool: Vec<f64>,          // numeric values: NodeKind::Num(idx) → nums_pool[idx]
    pub root: u32,
    /// Byte spans (within mmap) for lazy nodes; indexed by node.value_data.
    pub lazy_spans: Vec<LazySpan>,
    /// Estimated child count for each lazy span (for UI preview).
    pub lazy_child_counts: Vec<u16>,
    /// File path kept for on-demand lazy expansion (None for small files or in-memory parse).
    pub source_file: Option<String>,
    /// Shared mapping reused by lazy operations when the index was built from file.
    pub source_mmap: Option<Arc<memmap2::Mmap>>,
    /// Dynamic state for materialized lazy nodes.
    pub extra: Mutex<ExtraState>,
}

enum SourceBacking {
    Shared(Arc<memmap2::Mmap>),
    Owned(memmap2::Mmap),
}

impl SourceBacking {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Shared(mmap) => &mmap[..],
            Self::Owned(mmap) => &mmap[..],
        }
    }

    #[inline]
    fn into_arc(self) -> Arc<memmap2::Mmap> {
        match self {
            Self::Shared(mmap) => mmap,
            Self::Owned(mmap) => Arc::new(mmap),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HeapBytesBreakdown {
    pub nodes: usize,
    pub parent_index: usize,
    pub container_meta: usize, // kept for API compatibility: now covers both split Vecs
    pub keys: usize,
    pub val_strings: usize,
    pub nums_pool: usize,
}

impl HeapBytesBreakdown {
    #[inline]
    pub fn total(self) -> usize {
        self.nodes
            + self.parent_index
            + self.container_meta
            + self.keys
            + self.val_strings
            + self.nums_pool
    }
}

pub struct VisibleSliceRow {
    pub id: u32,
    pub depth: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ObjectSearchOperator {
    #[default]
    Contains,
    Equals,
    Regex,
    Exists,
}

#[derive(Debug, Clone, Default)]
pub struct ObjectSearchFilter {
    pub path: String,
    pub operator: ObjectSearchOperator,
    pub value: Option<String>,
    pub regex_case_insensitive: bool,
    pub regex_multiline: bool,
    pub regex_dot_all: bool,
}

struct CompiledObjectSearchFilter {
    path_segments: Vec<CompiledPathSegment>,
    operator: ObjectSearchOperator,
    value_cmp: Option<String>,
    value_num: Option<f64>,
    regex: Option<Regex>,
}

struct CompiledPathSegment {
    raw: String,
    lower: String,
    exact_id: Option<u32>,
    array_index: Option<u32>,
}

impl JsonIndex {
    fn source_backing(&self) -> Result<SourceBacking, String> {
        if let Some(mmap) = self.source_mmap.as_ref() {
            return Ok(SourceBacking::Shared(Arc::clone(mmap)));
        }

        let file_path = self
            .source_file
            .as_deref()
            .ok_or_else(|| "source_file not set".to_string())?;
        let file = File::open(file_path).map_err(|e| e.to_string())?;
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(|e| e.to_string())? };
        Ok(SourceBacking::Owned(mmap))
    }

    #[inline]
    fn children_len_for_node(&self, node: &Node) -> u32 {
        if node.kind().is_container() {
            self.container_children[node.value_data as usize]
        } else if node.kind().is_lazy() {
            self.lazy_child_counts[node.value_data as usize] as u32
        } else {
            0
        }
    }

    #[inline]
    fn subtree_len_for_node(&self, node: &Node) -> u32 {
        if node.kind().is_container() {
            self.container_subtrees[node.value_data as usize]
        } else {
            // Lazy nodes and leaves have subtree_len = 0 in the main index
            0
        }
    }

    #[inline]
    pub fn children_len(&self, id: u32) -> u32 {
        self.children_len_for_node(&self.nodes[id as usize])
    }

    #[inline]
    pub fn subtree_len(&self, id: u32) -> u32 {
        self.subtree_len_for_node(&self.nodes[id as usize])
    }

    #[inline]
    pub fn has_children(&self, id: u32) -> bool {
        self.children_len(id) != 0
    }

    // ---- Lazy expansion API ----

    /// Returns true if `id` refers to an extra (post-materialization) node.
    #[inline]
    pub fn is_extra_id(&self, id: u32) -> bool {
        let extra = self.extra.lock().unwrap();
        id >= extra.base
    }

    /// Parses the raw mmap bytes for a lazy node and stores its children in ExtraState.
    /// No-op if already materialized.
    pub fn materialize_lazy_node(&self, node_id: u32) -> Result<(), String> {
        let node = &self.nodes[node_id as usize];
        debug_assert!(
            node.kind().is_lazy(),
            "materialize_lazy_node called on non-lazy node"
        );

        // Check already materialized (fast path without sub-parse)
        {
            let extra = self.extra.lock().unwrap();
            if extra.mat.contains_key(&node_id) {
                return Ok(());
            }
        }

        let span_id = node.value_data as usize;
        let span = self.lazy_spans[span_id];

        let source = self
            .source_backing()
            .map_err(|_| "source_file not set for lazy expansion".to_string())?;
        let source_base = source.as_bytes().as_ptr() as usize;
        let start = span.file_offset as usize;
        let end = start + span.byte_len as usize;
        let raw = &source.as_bytes()[start..end];

        // Keep direct children eager for UX/search, but preserve lazy loading for
        // deeper nested containers so materializing one node doesn't eagerly parse
        // the entire subtree again.
        let mut sub_de = sonic_rs::Deserializer::from_slice(raw);
        let mut parsed = parse_streaming_with_cap(
            &mut sub_de,
            0,
            0,
            0,
            LAZY_DEPTH_THRESHOLD + 1,
            source_base,
        )
        .map_err(|e| e.to_string())?;
        parsed.source_file = self.source_file.clone();
        parsed.source_mmap = Some(source.into_arc());
        let sub_index = Arc::new(parsed);
        let sub_root = sub_index.root;

        let mut extra = self.extra.lock().unwrap();

        // Double-check after acquiring lock
        if extra.mat.contains_key(&node_id) {
            return Ok(());
        }

        let base = extra.base;
        let sub_idx = extra.sub_indices.len();

        // Collect direct children of sub_index root
        let child_sub_ids: Vec<u32> = sub_index.children_iter(sub_root).collect();

        // Register global IDs for direct children in extra_parent
        for (i, &sub_id) in child_sub_ids.iter().enumerate() {
            let global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id;
            extra.extra_parent.insert(global_id, node_id);
            // Also register grandchildren (children of containers within sub_index)
            // so parent_of_any works transitively for path building.
            let sub_node = &sub_index.nodes[sub_id as usize];
            if sub_node.kind().is_container() {
                Self::register_sub_descendants(
                    &sub_index,
                    sub_id,
                    global_id,
                    sub_idx,
                    base,
                    &mut extra.extra_parent,
                );
            }
            let _ = i; // suppress unused warning
        }

        extra.sub_indices.push(sub_index);
        extra.mat.insert(
            node_id,
            MatInfo2 {
                sub_idx,
                child_sub_ids,
            },
        );

        Ok(())
    }

    fn materialize_lazy_node_from_sub_index(
        &self,
        global_lazy_id: u32,
        owner_index: &JsonIndex,
        owner_lazy_id: u32,
    ) -> Result<Vec<u32>, String> {
        {
            let extra = self.extra.lock().unwrap();
            if let Some(mat) = extra.mat.get(&global_lazy_id) {
                let base = extra.base;
                let sub_idx = mat.sub_idx;
                return Ok(mat
                    .child_sub_ids
                    .iter()
                    .map(|&sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
                    .collect());
            }
        }

        let node = &owner_index.nodes[owner_lazy_id as usize];
        debug_assert!(node.kind().is_lazy(), "owner node must be lazy");

        let span_id = node.value_data as usize;
        let span = owner_index.lazy_spans[span_id];
        let source = owner_index.source_backing()?;
        let source_base = source.as_bytes().as_ptr() as usize;
        let start = span.file_offset as usize;
        let end = start + span.byte_len as usize;
        let raw = &source.as_bytes()[start..end];

        let mut sub_de = sonic_rs::Deserializer::from_slice(raw);
        let mut parsed = parse_streaming_with_cap(
            &mut sub_de,
            0,
            0,
            0,
            LAZY_DEPTH_THRESHOLD + 1,
            source_base,
        )
        .map_err(|e| e.to_string())?;
        parsed.source_file = owner_index.source_file.clone();
        parsed.source_mmap = Some(source.into_arc());
        let sub_index = Arc::new(parsed);
        let sub_root = sub_index.root;
        let child_sub_ids: Vec<u32> = sub_index.children_iter(sub_root).collect();

        let mut extra = self.extra.lock().unwrap();
        if let Some(mat) = extra.mat.get(&global_lazy_id) {
            let base = extra.base;
            let sub_idx = mat.sub_idx;
            return Ok(mat
                .child_sub_ids
                .iter()
                .map(|&sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
                .collect());
        }

        let base = extra.base;
        let sub_idx = extra.sub_indices.len();
        let mut child_ids = Vec::with_capacity(child_sub_ids.len());

        for &sub_id in &child_sub_ids {
            let global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id;
            extra.extra_parent.insert(global_id, global_lazy_id);
            let sub_node = &sub_index.nodes[sub_id as usize];
            if sub_node.kind().is_container() {
                Self::register_sub_descendants(
                    &sub_index,
                    sub_id,
                    global_id,
                    sub_idx,
                    base,
                    &mut extra.extra_parent,
                );
            }
            child_ids.push(global_id);
        }

        extra.sub_indices.push(sub_index);
        extra.mat.insert(
            global_lazy_id,
            MatInfo2 {
                sub_idx,
                child_sub_ids,
            },
        );

        Ok(child_ids)
    }

    /// Recursively registers extra_parent entries for sub-descendants.
    fn register_sub_descendants(
        sub_index: &JsonIndex,
        container_sub_id: u32,
        parent_global_id: u32,
        sub_idx: usize,
        base: u32,
        extra_parent: &mut HashMap<u32, u32>,
    ) {
        for child_sub_id in sub_index.children_iter(container_sub_id) {
            let child_global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + child_sub_id;
            extra_parent.insert(child_global_id, parent_global_id);
            let child_node = &sub_index.nodes[child_sub_id as usize];
            if child_node.kind().is_container() {
                Self::register_sub_descendants(
                    sub_index,
                    child_sub_id,
                    child_global_id,
                    sub_idx,
                    base,
                    extra_parent,
                );
            }
        }
    }

    /// Returns direct children of any node (main, lazy, or extra).
    /// For lazy nodes: materializes on demand.
    /// For extra nodes: returns their sub-children mapped to global IDs.
    pub fn get_children_any(&self, id: u32) -> Result<Vec<u32>, String> {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };

        if id < base {
            let node = &self.nodes[id as usize];
            if node.kind().is_lazy() {
                let span_id = node.value_data as usize;
                let is_large = span_id < self.lazy_spans.len()
                    && self.lazy_spans[span_id].byte_len >= LAZY_PAGINATE_THRESHOLD;
                if is_large {
                    // Use paginated scanner for large spans — avoids reading full span
                    return self.get_lazy_children_page(id, 0, EXPAND_SUBTREE_INLINE_PAGE);
                }
                self.materialize_lazy_node(id)?;
                let extra = self.extra.lock().unwrap();
                if let Some(mat) = extra.mat.get(&id) {
                    let sub_idx = mat.sub_idx;
                    let global_ids: Vec<u32> = mat
                        .child_sub_ids
                        .iter()
                        .map(|&sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
                        .collect();
                    return Ok(global_ids);
                }
                return Ok(Vec::new());
            }
            // Normal main-index container
            if node.kind().is_container() {
                return Ok(self.get_children_slice(id));
            }
            return Ok(Vec::new());
        }

        // Extra node: look up its sub-index
        let extra = self.extra.lock().unwrap();
        if let Some(mat) = extra.mat.get(&id) {
            let sub_idx = mat.sub_idx;
            let global_ids: Vec<u32> = mat
                .child_sub_ids
                .iter()
                .map(|&child_sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + child_sub_id)
                .collect();
            return Ok(global_ids);
        }
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;

        if sub_idx >= extra.sub_indices.len() {
            return Ok(Vec::new());
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);

        let sub_node = &sub_index.nodes[sub_id as usize];
        if sub_node.kind().is_lazy() {
            return self.materialize_lazy_node_from_sub_index(id, &sub_index, sub_id);
        }
        if !sub_node.kind().is_container() {
            return Ok(Vec::new());
        }
        let child_ids: Vec<u32> = sub_index
            .children_iter(sub_id)
            .map(|csub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + csub_id)
            .collect();
        Ok(child_ids)
    }

    /// Returns the parent of any node (main, lazy, or extra).
    pub fn parent_of_any(&self, id: u32) -> Option<u32> {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            return self.parent_of(id);
        }
        let extra = self.extra.lock().unwrap();
        extra.extra_parent.get(&id).copied()
    }

    /// Returns children count for any node (main, lazy, or extra).
    pub fn children_count_any(&self, id: u32) -> u32 {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            return self.children_len(id);
        }
        let extra = self.extra.lock().unwrap();
        if let Some(mat) = extra.mat.get(&id) {
            return mat.child_sub_ids.len() as u32;
        }
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return 0;
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        sub_index.children_len(sub_id)
    }

    /// Returns true if this is a lazy node with a large span (>= LAZY_PAGINATE_THRESHOLD).
    /// For such nodes the exact child count is unknown without a full scan.
    pub fn is_large_lazy(&self, id: u32) -> bool {
        let node = &self.nodes[id as usize];
        if !node.kind().is_lazy() {
            return false;
        }
        let span_id = node.value_data as usize;
        span_id < self.lazy_spans.len()
            && self.lazy_spans[span_id].byte_len >= LAZY_PAGINATE_THRESHOLD
    }

    /// Returns node kind and key for any node (main or extra).
    pub fn node_kind_any(&self, id: u32) -> NodeKind {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            return self.nodes[id as usize].kind();
        }
        let extra = self.extra.lock().unwrap();
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return NodeKind::Null;
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        sub_index.nodes[sub_id as usize].kind()
    }

    /// Returns raw JSON for a node. For lazy nodes reads directly from mmap.
    /// For extra nodes, rebuilds from the sub-index.
    pub fn get_raw_any(&self, id: u32) -> String {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            let node = &self.nodes[id as usize];
            if node.kind().is_lazy() {
                if let Ok(source) = self.source_backing() {
                    let span_id = node.value_data as usize;
                    if span_id < self.lazy_spans.len() {
                        let span = self.lazy_spans[span_id];
                        let start = span.file_offset as usize;
                        let end = start + span.byte_len as usize;
                        let mmap = source.as_bytes();
                        if end <= mmap.len() {
                            if let Ok(s) = std::str::from_utf8(&mmap[start..end]) {
                                return s.to_string();
                            }
                        }
                    }
                }
                return "null".to_string();
            }
            return self.build_raw(id);
        }

        // Extra node: rebuild from sub-index
        let extra = self.extra.lock().unwrap();
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return "null".to_string();
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        sub_index.build_raw(sub_id)
    }

    /// Returns path string for any node (main or extra).
    pub fn get_path_any(&self, node_id: u32) -> String {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if node_id < base {
            return self.get_path(node_id);
        }

        // Collect key segments by walking parent_of_any
        let mut segments: Vec<String> = Vec::with_capacity(16);
        let mut current = node_id;
        loop {
            let key_str = self.key_string_any(current);
            if let Some(k) = key_str {
                segments.push(k);
            }
            match self.parent_of_any(current) {
                None => break,
                Some(p) => current = p,
            }
        }
        segments.reverse();
        if segments.is_empty() {
            "$".to_string()
        } else {
            let mut out =
                String::with_capacity(segments.iter().map(|s| s.len() + 1).sum::<usize>() + 2);
            out.push('$');
            for seg in segments {
                out.push('.');
                out.push_str(&seg);
            }
            out
        }
    }

    /// Returns the key string for any node (main or extra).
    pub fn key_string_any(&self, id: u32) -> Option<String> {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            let node = &self.nodes[id as usize];
            return match node.key()? {
                NodeKey::String(kid) => Some(self.keys.get(kid).to_string()),
                NodeKey::ArrayIndex(idx) => Some(idx.to_string()),
            };
        }
        let extra = self.extra.lock().unwrap();
        // Check key override first (set e.g. for elements materialized from lazy spans during search).
        if let Some(k) = extra.extra_key_override.get(&id) {
            return Some(k.clone());
        }
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return None;
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        let node = &sub_index.nodes[sub_id as usize];
        match node.key()? {
            NodeKey::String(kid) => Some(sub_index.keys.get(kid).to_string()),
            NodeKey::ArrayIndex(idx) => Some(idx.to_string()),
        }
    }

    /// Returns a value preview string for any node (main or extra).
    pub fn value_preview_any(&self, id: u32) -> String {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            let node = &self.nodes[id as usize];
            let kind = node.kind();
            if kind.is_lazy() {
                let span_id = node.value_data as usize;
                let count = if span_id < self.lazy_child_counts.len() {
                    self.lazy_child_counts[span_id] as usize
                } else {
                    0
                };
                return if kind == NodeKind::LazyObject {
                    if count == 0 {
                        "{}".to_string()
                    } else {
                        format!("{{{} keys}}", count)
                    }
                } else {
                    if count == 0 {
                        "[]".to_string()
                    } else {
                        format!("[{} items]", count)
                    }
                };
            }
            // Delegate to existing method via a temporary node ref
            let children_len = self.children_len(id) as usize;
            return match kind {
                NodeKind::Object => {
                    if children_len == 0 {
                        "{}".to_string()
                    } else {
                        format!("{{{} keys}}", children_len)
                    }
                }
                NodeKind::Array => {
                    if children_len == 0 {
                        "[]".to_string()
                    } else {
                        format!("[{} items]", children_len)
                    }
                }
                NodeKind::Str => {
                    let s = self.str_val_of_node(node);
                    let truncated =
                        &s[..s.char_indices().nth(80).map(|(i, _)| i).unwrap_or(s.len())];
                    if truncated.len() < s.len() {
                        format!("\"{}…\"", truncated)
                    } else {
                        format!("\"{}\"", s)
                    }
                }
                NodeKind::Num => self.number_to_string(id),
                NodeKind::Bool => {
                    if node.value_data != 0 {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                }
                NodeKind::Null => "null".to_string(),
                _ => String::new(),
            };
        }

        let extra = self.extra.lock().unwrap();
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return "null".to_string();
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);

        let node = &sub_index.nodes[sub_id as usize];
        let children_len = sub_index.children_len(sub_id) as usize;
        match node.kind() {
            NodeKind::Object => {
                if children_len == 0 {
                    "{}".to_string()
                } else {
                    format!("{{{} keys}}", children_len)
                }
            }
            NodeKind::Array => {
                if children_len == 0 {
                    "[]".to_string()
                } else {
                    format!("[{} items]", children_len)
                }
            }
            NodeKind::Str => {
                let s = sub_index.str_val_of_node(node);
                let truncated = &s[..s.char_indices().nth(80).map(|(i, _)| i).unwrap_or(s.len())];
                if truncated.len() < s.len() {
                    format!("\"{}…\"", truncated)
                } else {
                    format!("\"{}\"", s)
                }
            }
            NodeKind::Num => sub_index.number_to_string(sub_id),
            NodeKind::Bool => {
                if node.value_data != 0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            NodeKind::Null => "null".to_string(),
            _ => String::new(),
        }
    }

    /// Returns the value_type string for any node (main or extra).
    pub fn value_type_any(&self, id: u32) -> &'static str {
        let kind = self.node_kind_any(id);
        match kind {
            NodeKind::Object => "object",
            NodeKind::Array => "array",
            NodeKind::Str => "string",
            NodeKind::Num => "number",
            NodeKind::Bool => "boolean",
            NodeKind::Null => "null",
            NodeKind::LazyObject => "object",
            NodeKind::LazyArray => "array",
        }
    }

    #[inline]
    fn number_as_f64(&self, node: &Node) -> f64 {
        debug_assert!(node.kind() == NodeKind::Num);
        if node.is_inline_num() {
            decode_inline_i31(node.value_data) as f64
        } else {
            self.nums_pool[node.value_data as usize]
        }
    }

    #[inline]
    fn format_number<'a, const N: usize>(
        &self,
        node: &Node,
        buf: &'a mut StackString<N>,
    ) -> &'a str {
        debug_assert!(node.kind() == NodeKind::Num);
        if node.is_inline_num() {
            buf.clear();
            write!(buf, "{}", decode_inline_i31(node.value_data))
                .expect("stack buffer too small for i31 formatting");
            buf.as_str()
        } else {
            format_f64_display(buf, self.nums_pool[node.value_data as usize])
        }
    }

    pub fn number_to_string(&self, id: u32) -> String {
        let node = &self.nodes[id as usize];
        let mut text = StackString::<64>::new();
        self.format_number(node, &mut text).to_string()
    }

    /// Returns the ids of direct children of `id`, computed from DFS preorder.
    /// First child = id+1; next sibling = prev_child + prev_child.subtree_len + 1.
    #[inline]
    pub fn get_children_slice(&self, id: u32) -> Vec<u32> {
        let count = self.children_len(id) as usize;
        let mut out = Vec::with_capacity(count);
        let mut cur = id + 1;
        for _ in 0..count {
            out.push(cur);
            cur += self.subtree_len(cur) + 1;
        }
        out
    }

    /// Zero-alloc iterator over direct children of `id`.
    #[inline]
    pub fn children_iter(&self, id: u32) -> ChildrenIter<'_> {
        ChildrenIter {
            nodes: &self.nodes,
            container_subtrees: &self.container_subtrees,
            cur: id + 1,
            remaining: self.children_len(id),
        }
    }

    pub fn heap_bytes_breakdown(&self) -> HeapBytesBreakdown {
        HeapBytesBreakdown {
            nodes: self.nodes.capacity() * std::mem::size_of::<Node>(),
            parent_index: self.parent_deltas.capacity() * std::mem::size_of::<u8>()
                + self.parent_overflow_ids.capacity() * std::mem::size_of::<u32>()
                + self.parent_overflow_values.capacity() * std::mem::size_of::<u32>(),
            container_meta: self.container_subtrees.capacity() * std::mem::size_of::<u32>()
                + self.container_children.capacity() * std::mem::size_of::<u32>(),
            keys: self.keys.heap_bytes_estimate(),
            val_strings: self.val_strings.heap_bytes_estimate(),
            nums_pool: self.nums_pool.capacity() * std::mem::size_of::<f64>(),
        }
    }

    pub fn heap_bytes_estimate(&self) -> usize {
        self.heap_bytes_breakdown().total()
    }

    /// Returns the direct parent of `id`.
    /// Fast path is O(1) via delta decoding; large parent gaps fall back to sparse overflow lookup.
    pub fn parent_of(&self, id: u32) -> Option<u32> {
        let delta = self.parent_deltas[id as usize];
        if delta == 0 {
            return None;
        }
        if delta != u8::MAX {
            return Some(id - delta as u32);
        }
        let Ok(overflow_idx) = self.parent_overflow_ids.binary_search(&id) else {
            return None; // dati inconsistenti — degrada senza crash
        };
        Some(self.parent_overflow_values[overflow_idx])
    }

    /// Returns the interned string value for a `NodeKind::Str` node.
    #[inline]
    pub fn str_val_of_node<'a>(&'a self, node: &Node) -> &'a str {
        debug_assert_eq!(node.kind(), NodeKind::Str);
        self.val_strings.get(node.value_data)
    }

    fn scoped_nodes(&self, scope_node_id: Option<u32>) -> (u32, &[Node]) {
        match scope_node_id {
            Some(scope_id) => {
                let start = scope_id as usize;
                let len = self.subtree_len(scope_id) as usize + 1;
                (scope_id, &self.nodes[start..start + len])
            }
            None => (0, &self.nodes),
        }
    }

    fn collect_matching_ids<F>(
        &self,
        start_id: u32,
        nodes: &[Node],
        max_results: usize,
        matches: F,
    ) -> Vec<u32>
    where
        F: Fn(u32, &Node) -> bool + Sync,
    {
        const CHUNK_SIZE: usize = 4096;

        if max_results == 0 || nodes.is_empty() {
            return Vec::new();
        }

        let chunk_matches: Vec<Vec<u32>> = nodes
            .par_chunks(CHUNK_SIZE)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let chunk_start = start_id + (chunk_idx * CHUNK_SIZE) as u32;
                let mut local = Vec::with_capacity(max_results.min(chunk.len()).min(32));
                for (offset, node) in chunk.iter().enumerate() {
                    let node_id = chunk_start + offset as u32;
                    if matches(node_id, node) {
                        local.push(node_id);
                        if local.len() == max_results {
                            break;
                        }
                    }
                }
                local
            })
            .collect();

        let mut ids = Vec::with_capacity(max_results.min(nodes.len()));
        for mut local in chunk_matches {
            let remaining = max_results - ids.len();
            if remaining == 0 {
                break;
            }
            if local.len() <= remaining {
                ids.append(&mut local);
            } else {
                ids.extend(local.into_iter().take(remaining));
                break;
            }
        }
        ids
    }

    pub fn from_str(json: &str) -> Result<Self, String> {
        // sonic-rs is SIMD-accelerated and ~2x faster than serde_json for in-memory strings.
        let mut de = sonic_rs::Deserializer::from_str(json);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, String> {
        let mut de = sonic_rs::Deserializer::from_slice(bytes);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    /// Loads from file via mmap + sonic-rs (SIMD parsing).
    ///
    /// The file is memory-mapped read-only for the duration of parsing: the OS
    /// demand-pages only the bytes actually needed and can evict already-parsed
    /// pages, so peak heap allocation is just the growing index (no 1 MB BufReader
    /// buffer). The mmap is released as soon as parsing finishes.
    ///
    /// # Safety
    /// The mapped file must not be modified externally while this function runs.
    /// This is the standard documented caveat for memory-mapped I/O.
    pub fn from_file(path: &str) -> Result<Self, String> {
        let file_size = std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| e.to_string())?;

        if file_size == 0 {
            return Err("file is empty".to_string());
        }

        Self::from_file_fast_lazy(path, file_size)
    }

    // ── Fast load helpers (forward+reverse byte scan) ─────────────────────────

    /// Modify `from_file` to use the fast path for very large files.
    /// Fast load for large files: reads only ~kilobytes total to build the lazy structure.
    /// Uses custom forward+reverse scan to find top-level value spans without sonic-rs.
    fn from_file_fast_lazy(path: &str, _file_size: u64) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(|e| e.to_string())? };
        let bytes = &mmap[..];

        let entries = scan_top_level_entries(bytes)?;

        // Determine root kind (object vs array)
        let mut root_start = 0usize;
        while root_start < bytes.len() && matches!(bytes[root_start], b' ' | b'\t' | b'\n' | b'\r')
        {
            root_start += 1;
        }
        let is_array_root = root_start < bytes.len() && bytes[root_start] == b'[';

        let mut ctx = StreamCtx::with_capacity(entries.len() + 2, 0);

        let root_kind = if is_array_root {
            NodeKind::Array
        } else {
            NodeKind::Object
        };
        let root_id = ctx.alloc(root_kind, None, 0, u32::MAX);

        for (i, entry) in entries.iter().enumerate() {
            let key = if is_array_root {
                Some(NodeKey::ArrayIndex(i as u32))
            } else {
                let kid = ctx.keys.intern(&entry.key);
                Some(NodeKey::String(kid))
            };

            if entry.is_container {
                let kind = if entry.is_array {
                    NodeKind::LazyArray
                } else {
                    NodeKind::LazyObject
                };
                let span_id = ctx.lazy_spans.len() as u32;
                let byte_len = (entry.value_end - entry.value_start) as u64;
                ctx.lazy_spans.push(LazySpan {
                    file_offset: entry.value_start as u64,
                    byte_len,
                });
                // Estimate children: 1 if non-empty, 0 if empty
                let placeholder: u16 = if byte_len > 2 { 1 } else { 0 };
                ctx.lazy_child_counts.push(placeholder);
                ctx.alloc_lazy(kind, key, span_id, root_id);
            } else {
                // Small scalar value: parse inline using sonic-rs
                let raw = &bytes[entry.value_start..entry.value_end];
                let mut sub_de = sonic_rs::Deserializer::from_slice(raw);
                let ctx_ptr = StreamCtxPtr::new(&mut ctx);
                let ctx_ptr: StreamCtxPtr<'static> = unsafe { std::mem::transmute(ctx_ptr) };
                let _ = ValSeed {
                    ctx: ctx_ptr,
                    parent: root_id,
                    key,
                    depth: 1,
                }
                .deserialize(&mut sub_de)
                .map_err(|e| e.to_string())?;
            }
        }

        // Set root subtree_len
        let meta_id = ctx.nodes[root_id as usize].value_data as usize;
        ctx.container_subtrees[meta_id] = ctx.nodes.len() as u32 - root_id - 1;
        ctx.container_children[meta_id] = entries.len() as u32;

        let mut index = finish_index(ctx);
        index.source_file = Some(path.to_string());
        index.source_mmap = Some(Arc::new(mmap));
        Ok(index)
    }

    /// Returns direct children global IDs for elements [offset..offset+limit] within a lazy
    /// container node, using a custom byte scanner. For small lazy nodes (< LAZY_PAGINATE_THRESHOLD)
    /// falls back to full materialization.
    pub fn get_lazy_children_page(
        &self,
        node_id: u32,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<u32>, String> {
        let node = &self.nodes[node_id as usize];
        debug_assert!(node.kind().is_lazy());

        let span_id = node.value_data as usize;
        let span = self.lazy_spans[span_id];

        // For small lazy spans, use full materialization (fast enough)
        if span.byte_len < LAZY_PAGINATE_THRESHOLD {
            self.materialize_lazy_node(node_id)?;
            let extra = self.extra.lock().unwrap();
            let base = extra.base;
            if let Some(mat) = extra.mat.get(&node_id) {
                let sub_idx = mat.sub_idx;
                let global_ids: Vec<u32> = mat
                    .child_sub_ids
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|&sub_id| base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
                    .collect();
                return Ok(global_ids);
            }
            return Ok(Vec::new());
        }

        {
            let extra = self.extra.lock().unwrap();
            if let Some(cached) = extra.page_cache.get(&(node_id, offset, limit)) {
                return Ok(cached.clone());
            }
        }

        // Large lazy span: use custom byte scanner for pagination
        let source = self
            .source_backing()
            .map_err(|_| "no source file for lazy expansion".to_string())?;
        let mmap = source.as_bytes();
        let start = span.file_offset as usize;
        let end = (span.file_offset + span.byte_len) as usize;
        let raw = &mmap[start..end.min(mmap.len())];
        let is_array = node.kind() == NodeKind::LazyArray;

        let elements = scan_json_elements(raw, is_array, offset, limit);
        if elements.is_empty() {
            return Ok(Vec::new());
        }

        // Build a wrapper JSON string for these elements and parse as a sub-index.
        // For arrays we use an object wrapper with keys = offset+i so that array
        // indices in the sub-index reflect the correct global position (e.g. page 2
        // at offset=1000 gets keys "1000", "1001", … instead of "0", "1", …).
        // For objects the keys are already embedded in the raw bytes.
        let total_bytes: usize = elements.iter().map(|(s, e)| e - s).sum::<usize>()
            + elements.len().saturating_sub(1) // commas
            + 2; // brackets
        let mut wrapper = String::with_capacity(total_bytes);
        wrapper.push('{');
        for (i, (elem_start, elem_end)) in elements.iter().enumerate() {
            if i > 0 {
                wrapper.push(',');
            }
            if is_array {
                // Explicit numeric key so the sub-index stores the correct global index.
                wrapper.push('"');
                wrapper.push_str(&(offset + i).to_string());
                wrapper.push_str("\":");
            }
            let elem_bytes = &raw[*elem_start..*elem_end];
            match std::str::from_utf8(elem_bytes) {
                Ok(s) => wrapper.push_str(s),
                Err(_) => wrapper.push_str("null"),
            }
        }
        wrapper.push('}');

        let sub_index = Arc::new(JsonIndex::from_str(&wrapper)?);
        let sub_root = sub_index.root;
        let child_sub_ids: Vec<u32> = sub_index.children_iter(sub_root).collect();

        let mut extra = self.extra.lock().unwrap();
        let base = extra.base;
        let sub_idx = extra.sub_indices.len();

        let mut child_ids = Vec::with_capacity(child_sub_ids.len());
        for &sub_id in &child_sub_ids {
            let global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id;
            extra.extra_parent.insert(global_id, node_id);
            let sub_node = &sub_index.nodes[sub_id as usize];
            if sub_node.kind().is_container() {
                Self::register_sub_descendants(
                    &sub_index,
                    sub_id,
                    global_id,
                    sub_idx,
                    base,
                    &mut extra.extra_parent,
                );
            }
            child_ids.push(global_id);
        }
        extra.sub_indices.push(sub_index);
        extra
            .page_cache
            .insert((node_id, offset, limit), child_ids.clone());

        Ok(child_ids)
    }

    /// Truly streaming parsing: reads from disk in chunks without buffering in RAM.
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, String> {
        let mut de = serde_json::Deserializer::from_reader(reader);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    pub fn expanded_visible_count(&self) -> usize {
        // main index count + all materialized sub-index descendant counts.
        let main_count = self.subtree_len(self.root) as usize;
        let extra = self.extra.lock().unwrap();
        let sub_count: usize = extra
            .sub_indices
            .iter()
            .map(|si| si.subtree_len(si.root) as usize)
            .sum();
        main_count + sub_count
    }

    /// DFS preorder traversal for the flat table view.
    /// Supports both main-index nodes and materialized sub-index nodes (lazy expansion).
    pub fn get_expanded_slice(&self, offset: usize, limit: usize) -> Vec<VisibleSliceRow> {
        if limit == 0 {
            return Vec::new();
        }

        // Tracks whether a frame is navigating the main index or a materialized sub-index.
        #[derive(Clone, Copy)]
        enum Src {
            Main,
            Sub(usize), // index into extra.sub_indices
        }

        struct Frame {
            src: Src,
            next_child_id: u32, // local ID within the relevant index
            remaining: u32,
            depth: usize,
        }

        let extra = self.extra.lock().unwrap();
        let mut stack: Vec<Frame> = Vec::new();
        let root_children_len = self.children_len(self.root);
        if root_children_len > 0 {
            stack.push(Frame {
                src: Src::Main,
                next_child_id: self.root + 1,
                remaining: root_children_len,
                depth: 0,
            });
        }

        let mut skipped = 0usize;
        let mut rows = Vec::with_capacity(limit.min(1024));

        'outer: loop {
            // Pop exhausted frames
            loop {
                match stack.last() {
                    None => break 'outer,
                    Some(f) if f.remaining > 0 => break,
                    _ => {
                        stack.pop();
                    }
                }
            }

            // Extract frame info and advance the frame pointer. The scoped block releases
            // the mutable borrow of `stack` before we push new frames below.
            let (local_cid, depth, src, child_subtree_len, child_children_len, global_id) = {
                let f = stack.last_mut().unwrap();
                let lcid = f.next_child_id;
                let depth = f.depth;
                let src = f.src;
                let (st, cl, gid) = match src {
                    Src::Main => {
                        let st = self.subtree_len(lcid);
                        let cl = self.children_len(lcid);
                        f.next_child_id = lcid + 1 + st;
                        f.remaining -= 1;
                        (st, cl, lcid)
                    }
                    Src::Sub(si_idx) => {
                        let si = &extra.sub_indices[si_idx];
                        let st = si.subtree_len(lcid);
                        let cl = si.children_len(lcid);
                        f.next_child_id = lcid + 1 + st;
                        f.remaining -= 1;
                        let gid = extra.base + (si_idx as u32) * SUB_INDEX_ID_RANGE + lcid;
                        (st, cl, gid)
                    }
                };
                (lcid, depth, src, st, cl, gid)
            }; // mutable borrow of stack released here

            if skipped < offset {
                // For lazy main-index nodes: the effective subtree size must include
                // materialized sub-index descendants, not just the lazy node placeholder
                // (which has subtree_len=0 in the main index).
                let subtree_size = match src {
                    Src::Main if self.nodes[local_cid as usize].kind().is_lazy() => {
                        if let Some(mat) = extra.mat.get(&local_cid) {
                            let si = &extra.sub_indices[mat.sub_idx];
                            1 + si.subtree_len(si.root) as usize
                        } else {
                            1
                        }
                    }
                    _ => child_subtree_len as usize + 1,
                };
                if skipped + subtree_size <= offset {
                    skipped += subtree_size;
                    continue;
                }
                skipped += 1;
                // Push children to continue scanning into the skipped subtree
                match src {
                    Src::Main => {
                        let node = &self.nodes[local_cid as usize];
                        if node.kind().is_lazy() {
                            if let Some(mat) = extra.mat.get(&local_cid) {
                                let si_idx = mat.sub_idx;
                                let si = &extra.sub_indices[si_idx];
                                let cl = si.children_len(si.root);
                                if cl > 0 {
                                    stack.push(Frame {
                                        src: Src::Sub(si_idx),
                                        next_child_id: si.root + 1,
                                        remaining: cl,
                                        depth: depth + 1,
                                    });
                                }
                            }
                        } else if child_children_len > 0 {
                            stack.push(Frame {
                                src: Src::Main,
                                next_child_id: local_cid + 1,
                                remaining: child_children_len,
                                depth: depth + 1,
                            });
                        }
                    }
                    Src::Sub(si_idx) => {
                        if child_children_len > 0 {
                            stack.push(Frame {
                                src: Src::Sub(si_idx),
                                next_child_id: local_cid + 1,
                                remaining: child_children_len,
                                depth: depth + 1,
                            });
                        }
                    }
                }
                continue;
            }

            rows.push(VisibleSliceRow {
                id: global_id,
                depth,
            });
            if rows.len() >= limit {
                break 'outer;
            }

            // Push children frame
            match src {
                Src::Main => {
                    let node = &self.nodes[local_cid as usize];
                    if node.kind().is_lazy() {
                        if let Some(mat) = extra.mat.get(&local_cid) {
                            let si_idx = mat.sub_idx;
                            let si = &extra.sub_indices[si_idx];
                            let cl = si.children_len(si.root);
                            if cl > 0 {
                                stack.push(Frame {
                                    src: Src::Sub(si_idx),
                                    next_child_id: si.root + 1,
                                    remaining: cl,
                                    depth: depth + 1,
                                });
                            }
                        }
                    } else if child_children_len > 0 {
                        stack.push(Frame {
                            src: Src::Main,
                            next_child_id: local_cid + 1,
                            remaining: child_children_len,
                            depth: depth + 1,
                        });
                    }
                }
                Src::Sub(si_idx) => {
                    if child_children_len > 0 {
                        stack.push(Frame {
                            src: Src::Sub(si_idx),
                            next_child_id: local_cid + 1,
                            remaining: child_children_len,
                            depth: depth + 1,
                        });
                    }
                }
            }
        }

        rows
    }

    /// Costruisce la rappresentazione JSON raw di un nodo, iterativamente (no ricorsione).
    /// For lazy nodes, returns the raw mmap bytes directly.
    pub fn build_raw(&self, start_id: u32) -> String {
        // Fast path for lazy nodes: return mmap bytes directly
        if start_id < self.nodes.len() as u32 {
            let node = &self.nodes[start_id as usize];
            if node.kind().is_lazy() {
                return self.get_raw_any(start_id);
            }
        }
        // Forward DFS using an explicit stack of frames.
        // Each frame tracks iteration state over a container's children,
        // eliminating the per-node Vec<u32> allocation of the old approach.
        struct Frame {
            next_child_id: u32, // DFS id of the next child to emit
            remaining: u32,     // children still to emit
            is_object: bool,
        }

        let mut out = String::with_capacity(256);
        let mut stack: Vec<Frame> = Vec::with_capacity(32);
        let mut current = start_id;

        // Emit `current`, then loop: advance to next sibling or pop frame.
        loop {
            let node = &self.nodes[current as usize];

            match node.kind() {
                NodeKind::Object | NodeKind::Array => {
                    let is_object = node.kind() == NodeKind::Object;
                    let count = self.children_len(current);
                    if count == 0 {
                        out.push_str(if is_object { "{}" } else { "[]" });
                    } else {
                        out.push(if is_object { '{' } else { '[' });
                        let first = current + 1;
                        // Emit key of first child if object
                        if is_object {
                            if let Some(kid) = self.nodes[first as usize].string_key_id() {
                                out.push('"');
                                json_escape_into(&mut out, self.keys.get(kid));
                                out.push_str("\":");
                            }
                        }
                        let first_subtree = self.subtree_len(first);
                        stack.push(Frame {
                            next_child_id: first + 1 + first_subtree,
                            remaining: count - 1,
                            is_object,
                        });
                        current = first;
                        continue; // emit first child next iteration
                    }
                }
                NodeKind::Str => {
                    out.push('"');
                    json_escape_into(&mut out, self.str_val_of_node(node));
                    out.push('"');
                }
                NodeKind::Num => {
                    let mut text = StackString::<64>::new();
                    out.push_str(self.format_number(node, &mut text));
                }
                NodeKind::Bool => {
                    out.push_str(if node.value_data != 0 {
                        "true"
                    } else {
                        "false"
                    });
                }
                NodeKind::Null => {
                    out.push_str("null");
                }
                NodeKind::LazyObject | NodeKind::LazyArray => {
                    // Emit raw mmap bytes for this lazy node directly
                    let lazy_raw = self.get_raw_any(current);
                    out.push_str(&lazy_raw);
                }
            }

            // Advance: find next sibling in the nearest non-exhausted frame.
            loop {
                match stack.last_mut() {
                    None => return out,
                    Some(frame) => {
                        if frame.remaining == 0 {
                            out.push(if frame.is_object { '}' } else { ']' });
                            stack.pop();
                            // continue popping / or find next sibling in outer frame
                        } else {
                            out.push(',');
                            let next = frame.next_child_id;
                            if frame.is_object {
                                if let Some(kid) = self.nodes[next as usize].string_key_id() {
                                    out.push('"');
                                    json_escape_into(&mut out, self.keys.get(kid));
                                    out.push_str("\":");
                                }
                            }
                            frame.next_child_id = next + 1 + self.subtree_len(next);
                            frame.remaining -= 1;
                            current = next;
                            break;
                        }
                    }
                }
            }
        }
    }

    fn path_segment_len(&self, key: NodeKey) -> usize {
        match key {
            NodeKey::String(id) => self.keys.get(id).len(),
            NodeKey::ArrayIndex(index) => decimal_len_u32(index),
        }
    }

    fn push_path_segment(&self, out: &mut String, key: NodeKey) {
        match key {
            NodeKey::String(id) => out.push_str(self.keys.get(id)),
            NodeKey::ArrayIndex(index) => {
                let mut buf = [0u8; 10];
                out.push_str(format_u32_decimal(&mut buf, index));
            }
        }
    }

    fn node_key_as_str<'a>(&'a self, node: &'a Node, buf: &'a mut [u8; 10]) -> Option<&'a str> {
        match node.key()? {
            NodeKey::String(id) => Some(self.keys.get(id)),
            NodeKey::ArrayIndex(index) => Some(format_u32_decimal(buf, index)),
        }
    }

    pub fn get_path(&self, node_id: u32) -> String {
        let mut key_ids: Vec<NodeKey> = Vec::with_capacity(16);
        let mut current = node_id;
        loop {
            let node = &self.nodes[current as usize];
            if let Some(key) = node.key() {
                key_ids.push(key);
            }
            match self.parent_of(current) {
                None => break,
                Some(p) => current = p,
            }
        }
        key_ids.reverse();
        if key_ids.is_empty() {
            "$".to_string()
        } else {
            let total_len: usize = key_ids
                .iter()
                .map(|&key| self.path_segment_len(key))
                .sum::<usize>()
                + key_ids.len()
                + 2;
            let mut out = String::with_capacity(total_len);
            out.push('$');
            for key in key_ids {
                out.push('.');
                self.push_path_segment(&mut out, key);
            }
            out
        }
    }

    fn key_matches_regex(&self, node: &Node, re: &Regex) -> bool {
        let mut key_buf = [0u8; 10];
        self.node_key_as_str(node, &mut key_buf)
            .is_some_and(|key| re.is_match(key))
    }

    fn key_matches_query(
        &self,
        node: &Node,
        query: &str,
        query_lower: &str,
        case_sensitive: bool,
        exact_match: bool,
        exact_array_index: Option<u32>,
    ) -> bool {
        match node.key() {
            Some(NodeKey::String(id)) => matches_text(
                self.keys.get(id),
                query,
                query_lower,
                case_sensitive,
                exact_match,
            ),
            Some(NodeKey::ArrayIndex(index)) => {
                if exact_match {
                    return exact_array_index == Some(index);
                }
                let mut key_buf = [0u8; 10];
                matches_text(
                    format_u32_decimal(&mut key_buf, index),
                    query,
                    query_lower,
                    case_sensitive,
                    false,
                )
            }
            None => false,
        }
    }

    fn value_matches_regex(&self, node: &Node, re: &Regex) -> bool {
        match node.kind() {
            NodeKind::Str => re.is_match(self.str_val_of_node(node)),
            NodeKind::Num => {
                let mut text = StackString::<64>::new();
                re.is_match(self.format_number(node, &mut text))
            }
            NodeKind::Bool => re.is_match(if node.value_data != 0 {
                "true"
            } else {
                "false"
            }),
            NodeKind::Null => re.is_match("null"),
            NodeKind::Object | NodeKind::Array | NodeKind::LazyObject | NodeKind::LazyArray => {
                false
            }
        }
    }

    fn value_matches_query(
        &self,
        node: &Node,
        query: &str,
        query_lower: &str,
        case_sensitive: bool,
        exact_match: bool,
        exact_number: Option<f64>,
    ) -> bool {
        match node.kind() {
            NodeKind::Str => matches_text(
                self.str_val_of_node(node),
                query,
                query_lower,
                case_sensitive,
                exact_match,
            ),
            NodeKind::Num => {
                let value = self.number_as_f64(node);
                if exact_match {
                    return exact_number.is_some_and(|expected| value == expected);
                }
                let mut text = StackString::<64>::new();
                matches_text(
                    self.format_number(node, &mut text),
                    query,
                    query_lower,
                    case_sensitive,
                    false,
                )
            }
            NodeKind::Bool => matches_text(
                if node.value_data != 0 {
                    "true"
                } else {
                    "false"
                },
                query,
                query_lower,
                case_sensitive,
                exact_match,
            ),
            NodeKind::Null => matches_text("null", query, query_lower, case_sensitive, exact_match),
            NodeKind::Object | NodeKind::Array | NodeKind::LazyObject | NodeKind::LazyArray => {
                false
            }
        }
    }

    fn value_matches_regex_any(&self, id: u32, re: &Regex) -> bool {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            return self.value_matches_regex(&self.nodes[id as usize], re);
        }
        let extra = self.extra.lock().unwrap();
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return false;
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        sub_index.value_matches_regex(&sub_index.nodes[sub_id as usize], re)
    }

    fn value_matches_query_any(
        &self,
        id: u32,
        query: &str,
        query_lower: &str,
        case_sensitive: bool,
        exact_match: bool,
        exact_number: Option<f64>,
    ) -> bool {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        if id < base {
            return self.value_matches_query(
                &self.nodes[id as usize],
                query,
                query_lower,
                case_sensitive,
                exact_match,
                exact_number,
            );
        }
        let extra = self.extra.lock().unwrap();
        let inner = id - base;
        let sub_idx = (inner / SUB_INDEX_ID_RANGE) as usize;
        let sub_id = inner % SUB_INDEX_ID_RANGE;
        if sub_idx >= extra.sub_indices.len() {
            return false;
        }
        let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
        drop(extra);
        sub_index.value_matches_query(
            &sub_index.nodes[sub_id as usize],
            query,
            query_lower,
            case_sensitive,
            exact_match,
            exact_number,
        )
    }

    pub fn search(
        &self,
        query: &str,
        target: &str,
        case_sensitive: bool,
        use_regex: bool,
        exact_match: bool,
        max_results: usize,
        path: Option<&str>,
        multiline: bool,
        dot_all: bool,
    ) -> Vec<u32> {
        let scope_node_id = match path.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => match self.resolve_path(path) {
                Some(node_id) => Some(node_id),
                None => return vec![],
            },
            None => None,
        };

        let want_keys = target == "keys" || target == "both";
        let want_values = target == "values" || target == "both";
        let (start_id, scoped_nodes) = self.scoped_nodes(scope_node_id);

        if use_regex {
            let mut flags = String::new();
            if !case_sensitive {
                flags.push('i');
            }
            if multiline {
                flags.push('m');
            }
            if dot_all {
                flags.push('s');
            }
            let pattern = if flags.is_empty() {
                query.to_string()
            } else {
                format!("(?{}){}", flags, query)
            };
            let re = match Regex::new(&pattern) {
                Ok(r) => r,
                Err(_) => return vec![],
            };
            self.collect_matching_ids(start_id, scoped_nodes, max_results, |_, node| {
                if want_keys && !want_values && node.key().is_none() {
                    return false;
                }
                let matches_key = want_keys && self.key_matches_regex(node, &re);
                let matches_value = want_values && self.value_matches_regex(node, &re);
                matches_key || matches_value
            })
        } else {
            let query_lower = if case_sensitive {
                String::new()
            } else {
                query.to_lowercase()
            };
            let exact_number = if exact_match {
                query.parse::<f64>().ok()
            } else {
                None
            };
            let exact_array_index = if exact_match {
                query.parse::<u32>().ok()
            } else {
                None
            };

            // Fast-path: exact case-sensitive key lookup (O(1) via hash table).
            if want_keys && !want_values && exact_match && case_sensitive {
                let target_string_key = self.keys.id_of(query);
                return self.collect_matching_ids(
                    start_id,
                    scoped_nodes,
                    max_results,
                    |_, node| match node.key() {
                        Some(NodeKey::String(id)) => target_string_key == Some(id),
                        Some(NodeKey::ArrayIndex(index)) => exact_array_index == Some(index),
                        None => false,
                    },
                );
            }

            // Fast-path: case-sensitive substring — use memmem (SIMD) built once.
            // Kept separate so the common case-insensitive path has zero Option overhead.
            if case_sensitive && !exact_match {
                let finder = memmem::Finder::new(query.as_bytes());
                return self.collect_matching_ids(
                    start_id,
                    scoped_nodes,
                    max_results,
                    |_, node| {
                        if want_keys && !want_values && node.key().is_none() {
                            return false;
                        }
                        let matches_key = want_keys
                            && match node.key() {
                                Some(NodeKey::String(id)) => {
                                    finder.find(self.keys.get(id).as_bytes()).is_some()
                                }
                                Some(NodeKey::ArrayIndex(idx)) => {
                                    let mut buf = [0u8; 10];
                                    finder
                                        .find(format_u32_decimal(&mut buf, idx).as_bytes())
                                        .is_some()
                                }
                                None => false,
                            };
                        if matches_key {
                            return true;
                        }
                        want_values
                            && match node.kind() {
                                NodeKind::Str => {
                                    finder.find(self.str_val_of_node(node).as_bytes()).is_some()
                                }
                                NodeKind::Num => {
                                    let mut text = StackString::<64>::new();
                                    finder
                                        .find(self.format_number(node, &mut text).as_bytes())
                                        .is_some()
                                }
                                NodeKind::Bool => finder
                                    .find(if node.value_data != 0 {
                                        b"true"
                                    } else {
                                        b"false"
                                    })
                                    .is_some(),
                                NodeKind::Null => finder.find(b"null").is_some(),
                                NodeKind::Object
                                | NodeKind::Array
                                | NodeKind::LazyObject
                                | NodeKind::LazyArray => false,
                            }
                    },
                );
            }

            // Default path: case-insensitive or exact-match (no finder overhead).
            self.collect_matching_ids(start_id, scoped_nodes, max_results, |_, node| {
                if want_keys && !want_values && node.key().is_none() {
                    return false;
                }
                if want_values
                    && !want_keys
                    && matches!(
                        node.kind(),
                        NodeKind::Object
                            | NodeKind::Array
                            | NodeKind::LazyObject
                            | NodeKind::LazyArray
                    )
                {
                    return false;
                }
                let matches_key = want_keys
                    && self.key_matches_query(
                        node,
                        query,
                        &query_lower,
                        case_sensitive,
                        exact_match,
                        exact_array_index,
                    );
                let matches_value = want_values
                    && self.value_matches_query(
                        node,
                        query,
                        &query_lower,
                        case_sensitive,
                        exact_match,
                        exact_number,
                    );
                matches_key || matches_value
            })
        }
    }

    pub fn resolve_path(&self, path: &str) -> Option<u32> {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "$" {
            return Some(self.root);
        }

        let normalized = trimmed
            .strip_prefix("$.")
            .or_else(|| trimmed.strip_prefix('$'))?;
        if normalized.is_empty() {
            return Some(self.root);
        }

        let mut current = self.root;
        for segment in normalized.split('.').filter(|segment| !segment.is_empty()) {
            let parent = &self.nodes[current as usize];
            current = match parent.kind() {
                NodeKind::Array => {
                    let wanted_index = segment.parse::<u32>().ok()?;
                    self.children_iter(current).find(|&child_id| {
                        self.nodes[child_id as usize].array_index() == Some(wanted_index)
                    })?
                }
                NodeKind::Object => self.children_iter(current).find(|&child_id| {
                    self.nodes[child_id as usize]
                        .string_key_id()
                        .is_some_and(|kid| self.keys.get(kid) == segment)
                })?,
                _ => return None,
            };
        }

        Some(current)
    }

    fn find_child_any_by_segment(&self, parent_id: u32, segment: &str) -> Option<u32> {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };

        if parent_id < base {
            let parent = &self.nodes[parent_id as usize];
            return match parent.kind() {
                NodeKind::Array => {
                    let wanted_index = segment.parse::<u32>().ok()?;
                    self.children_iter(parent_id).find(|&child_id| {
                        self.nodes[child_id as usize].array_index() == Some(wanted_index)
                    })
                }
                NodeKind::Object => self.children_iter(parent_id).find(|&child_id| {
                    self.nodes[child_id as usize]
                        .string_key_id()
                        .is_some_and(|kid| self.keys.get(kid) == segment)
                }),
                NodeKind::LazyArray => {
                    let wanted_index = segment.parse::<usize>().ok()?;
                    if self.is_large_lazy(parent_id) {
                        return self
                            .get_lazy_children_page(parent_id, wanted_index, 1)
                            .ok()?
                            .into_iter()
                            .next();
                    }
                    self.get_children_any(parent_id)
                        .ok()?
                        .into_iter()
                        .find(|&child_id| {
                            self.key_string_any(child_id)
                                .is_some_and(|key| key == segment)
                        })
                }
                NodeKind::LazyObject => {
                    if self.is_large_lazy(parent_id) {
                        self.materialize_lazy_node(parent_id).ok()?;
                    }
                    self.get_children_any(parent_id)
                        .ok()?
                        .into_iter()
                        .find(|&child_id| {
                            self.key_string_any(child_id)
                                .is_some_and(|key| key == segment)
                        })
                }
                _ => None,
            };
        }

        self.get_children_any(parent_id)
            .ok()?
            .into_iter()
            .find(|&child_id| {
                self.key_string_any(child_id)
                    .is_some_and(|key| key == segment)
            })
    }

    pub fn resolve_path_any(&self, path: &str) -> Option<u32> {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "$" {
            return Some(self.root);
        }

        let normalized = trimmed
            .strip_prefix("$.")
            .or_else(|| trimmed.strip_prefix('$'))?;
        if normalized.is_empty() {
            return Some(self.root);
        }

        let mut current = self.root;
        for segment in normalized.split('.').filter(|segment| !segment.is_empty()) {
            current = self.find_child_any_by_segment(current, segment)?;
        }

        Some(current)
    }

    fn compile_object_search_filters(
        &self,
        filters: &[ObjectSearchFilter],
        _key_case_sensitive: bool,
        value_case_sensitive: bool,
    ) -> Option<Vec<CompiledObjectSearchFilter>> {
        let mut compiled = Vec::with_capacity(filters.len());
        for filter in filters {
            let path = filter.path.trim();
            if path.is_empty() {
                return None;
            }

            let mut path_segments = Vec::new();
            for segment in path.split('.').filter(|segment| !segment.is_empty()) {
                let exact_id = self.keys.id_of(segment);
                let array_index = segment.parse::<u32>().ok();
                path_segments.push(CompiledPathSegment {
                    raw: segment.to_string(),
                    lower: segment.to_lowercase(),
                    exact_id,
                    array_index,
                });
            }
            if path_segments.is_empty() {
                return None;
            }

            let value = filter.value.as_ref().map(|value| value.trim().to_string());
            let value_cmp = match filter.operator {
                ObjectSearchOperator::Exists => None,
                ObjectSearchOperator::Regex => None,
                ObjectSearchOperator::Contains | ObjectSearchOperator::Equals => {
                    let value = value.as_ref()?;
                    Some(if value_case_sensitive {
                        value.clone()
                    } else {
                        value.to_lowercase()
                    })
                }
            };
            let value_num = if filter.operator == ObjectSearchOperator::Equals {
                value.as_deref().and_then(|raw| raw.parse::<f64>().ok())
            } else {
                None
            };
            let regex = match filter.operator {
                ObjectSearchOperator::Regex => {
                    let pattern = value.as_ref()?;
                    let mut flags = String::new();
                    if filter.regex_case_insensitive || !value_case_sensitive {
                        flags.push('i');
                    }
                    if filter.regex_multiline {
                        flags.push('m');
                    }
                    if filter.regex_dot_all {
                        flags.push('s');
                    }
                    let pattern = if flags.is_empty() {
                        pattern.clone()
                    } else {
                        format!("(?{flags}){pattern}")
                    };
                    Some(Regex::new(&pattern).ok()?)
                }
                _ => None,
            };

            compiled.push(CompiledObjectSearchFilter {
                path_segments,
                operator: filter.operator.clone(),
                value_cmp,
                value_num,
                regex,
            });
        }
        Some(compiled)
    }

    fn find_child_any_by_compiled_segment(
        &self,
        parent_id: u32,
        segment: &CompiledPathSegment,
        key_case_sensitive: bool,
    ) -> Option<u32> {
        let base = {
            let extra = self.extra.lock().unwrap();
            extra.base
        };
        let kind = self.node_kind_any(parent_id);
        match kind {
            NodeKind::Array | NodeKind::LazyArray => {
                let wanted_index = segment.array_index?;
                if kind == NodeKind::LazyArray && parent_id < base && self.is_large_lazy(parent_id) {
                    return self
                        .get_lazy_children_page(parent_id, wanted_index as usize, 1)
                        .ok()?
                        .into_iter()
                        .next();
                }
                self.get_children_any(parent_id)
                    .ok()?
                    .into_iter()
                    .find(|&child_id| {
                        self.key_string_any(child_id)
                            .is_some_and(|key| key == wanted_index.to_string())
                    })
            }
            NodeKind::Object | NodeKind::LazyObject => {
                self.get_children_any(parent_id)
                    .ok()?
                    .into_iter()
                    .find(|&child_id| {
                        let Some(child_key) = self.key_string_any(child_id) else {
                            return false;
                        };
                        if key_case_sensitive {
                            if let Some(exact_id) = segment.exact_id {
                                if child_id < base {
                                    return self.nodes[child_id as usize].string_key_id()
                                        == Some(exact_id);
                                }
                            }
                            child_key == segment.raw
                        } else {
                            matches_text(&child_key, &segment.raw, &segment.lower, false, true)
                        }
                    })
            }
            _ => None,
        }
    }

    fn resolve_relative_path(
        &self,
        start_node_id: u32,
        path_segments: &[CompiledPathSegment],
        key_case_sensitive: bool,
    ) -> Option<u32> {
        let mut current = start_node_id;
        for segment in path_segments {
            current =
                self.find_child_any_by_compiled_segment(current, segment, key_case_sensitive)?;
        }
        Some(current)
    }

    fn object_filter_matches(
        &self,
        object_id: u32,
        filter: &CompiledObjectSearchFilter,
        key_case_sensitive: bool,
        value_case_sensitive: bool,
    ) -> bool {
        let Some(target_id) =
            self.resolve_relative_path(object_id, &filter.path_segments, key_case_sensitive)
        else {
            return false;
        };

        match filter.operator {
            ObjectSearchOperator::Exists => true,
            ObjectSearchOperator::Regex => filter
                .regex
                .as_ref()
                .is_some_and(|regex| self.value_matches_regex_any(target_id, regex)),
            ObjectSearchOperator::Contains | ObjectSearchOperator::Equals => {
                let needle = filter.value_cmp.as_deref().unwrap_or_default();
                self.value_matches_query_any(
                    target_id,
                    needle,
                    needle,
                    value_case_sensitive,
                    filter.operator == ObjectSearchOperator::Equals,
                    filter.value_num,
                )
            }
        }
    }

    pub fn search_objects_in_lazy_node(
        &self,
        lazy_node_id: u32,
        filters: &[ObjectSearchFilter],
        key_case_sensitive: bool,
        value_case_sensitive: bool,
        max_results: usize,
    ) -> Result<Vec<u32>, String> {
        let node = &self.nodes[lazy_node_id as usize];
        if !node.kind().is_lazy() || max_results == 0 {
            return Ok(Vec::new());
        }

        let is_large_array = node.kind() == NodeKind::LazyArray && self.is_large_lazy(lazy_node_id);
        if !is_large_array {
            self.materialize_lazy_node(lazy_node_id)?;

            let extra = self.extra.lock().unwrap();
            let base = extra.base;
            let Some(mat) = extra.mat.get(&lazy_node_id) else {
                return Ok(Vec::new());
            };
            let sub_idx = mat.sub_idx;
            let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
            drop(extra);

            let matches = sub_index.search_objects(
                filters,
                key_case_sensitive,
                value_case_sensitive,
                max_results,
                None,
            );

            return Ok(matches
                .into_iter()
                .map(|sub_id| {
                    if sub_id == sub_index.root {
                        lazy_node_id
                    } else {
                        base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id
                    }
                })
                .collect());
        }

        let span_id = node.value_data as usize;
        if span_id >= self.lazy_spans.len() {
            return Ok(Vec::new());
        }
        let span = self.lazy_spans[span_id];

        let source = self.source_backing()?;
        let mmap = source.as_bytes();
        let span_start = span.file_offset as usize;
        let span_end = span_start + span.byte_len as usize;
        let bytes = &mmap[span_start..span_end.min(mmap.len())];

        let mut results: Vec<u32> = Vec::new();
        let mut elem_index: usize = 0;
        let mut pos = 0usize;
        let base = { self.extra.lock().unwrap().base };

        while pos < bytes.len() && bytes[pos] != b'[' {
            pos += 1;
        }
        if pos >= bytes.len() {
            return Ok(results);
        }
        pos += 1;

        loop {
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b',') {
                pos += 1;
            }
            if pos >= bytes.len() || bytes[pos] == b']' {
                break;
            }

            let value_start = pos;
            pos = scan_json_value_end(bytes, pos);
            let elem_end = pos.min(bytes.len());
            let elem_bytes = &bytes[value_start..elem_end];

            if let Ok(sub_index) = JsonIndex::from_slice(elem_bytes) {
                let matching_sub = sub_index.search_objects(
                    filters,
                    key_case_sensitive,
                    value_case_sensitive,
                    max_results.saturating_sub(results.len()),
                    None,
                );

                if !matching_sub.is_empty() {
                    let sub_index = Arc::new(sub_index);
                    let sub_root = sub_index.root;
                    let mut extra = self.extra.lock().unwrap();
                    let sub_idx = extra.sub_indices.len();
                    let root_global = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_root;

                    extra.extra_parent.insert(root_global, lazy_node_id);
                    extra
                        .extra_key_override
                        .insert(root_global, elem_index.to_string());

                    Self::register_sub_descendants(
                        &sub_index,
                        sub_root,
                        root_global,
                        sub_idx,
                        base,
                        &mut extra.extra_parent,
                    );

                    extra.sub_indices.push(sub_index);

                    for sub_id in matching_sub {
                        let global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id;
                        results.push(global_id);
                        if results.len() >= max_results {
                            break;
                        }
                    }
                }
            }

            elem_index += 1;
            if results.len() >= max_results {
                break;
            }
        }

        Ok(results)
    }

    pub fn search_objects(
        &self,
        filters: &[ObjectSearchFilter],
        key_case_sensitive: bool,
        value_case_sensitive: bool,
        max_results: usize,
        path: Option<&str>,
    ) -> Vec<u32> {
        if filters.is_empty() || max_results == 0 {
            return vec![];
        }

        let scope_node_id = match path.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => match self.resolve_path(path) {
                Some(node_id) => Some(node_id),
                None => return vec![],
            },
            None => None,
        };

        let Some(compiled_filters) =
            self.compile_object_search_filters(filters, key_case_sensitive, value_case_sensitive)
        else {
            return vec![];
        };
        let (start_id, scoped_nodes) = self.scoped_nodes(scope_node_id);

        let mut results =
            self.collect_matching_ids(start_id, scoped_nodes, max_results, |node_id, node| {
                node.kind() == NodeKind::Object
                    && compiled_filters.iter().all(|filter| {
                        self.object_filter_matches(
                            node_id,
                            filter,
                            key_case_sensitive,
                            value_case_sensitive,
                        )
                    })
            });

        // Search inside lazy nodes separately so scope and result IDs stay coherent.
        if results.len() < max_results {
            let lazy_ids: Vec<u32> = scoped_nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.kind().is_lazy())
                .map(|(i, _)| start_id + i as u32)
                .collect();

            for lazy_id in lazy_ids {
                if results.len() >= max_results {
                    break;
                }
                if let Ok(lazy_results) = self.search_objects_in_lazy_node(
                    lazy_id,
                    filters,
                    key_case_sensitive,
                    value_case_sensitive,
                    max_results - results.len(),
                ) {
                    results.extend(lazy_results);
                    if results.len() >= max_results {
                        break;
                    }
                }
            }
        }

        results
    }

    pub fn suggest_property_paths(&self, prefix: &str, limit: usize) -> Vec<String> {
        let trimmed = prefix.trim();
        if limit == 0 {
            return vec![];
        }

        let (base, segment_prefix) = match trimmed.rfind('.') {
            Some(idx) => (&trimmed[..=idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };
        let prefix_lower = segment_prefix.to_lowercase();

        // Collect unique matching keys from the main index and all materialized sub-indices.
        // With always-lazy loading the main index only contains top-level keys; nested
        // object keys live inside sub-indices created during lazy node materialization.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        let collect = |keys: &InternedStrings, seen: &mut std::collections::HashSet<String>| {
            for id in 0..keys.len() as u32 {
                let candidate = keys.get(id);
                if starts_with_case_insensitive(candidate, segment_prefix, &prefix_lower) {
                    seen.insert(candidate.to_string());
                }
            }
        };

        collect(&self.keys, &mut seen);

        {
            let extra = self.extra.lock().unwrap();
            for sub_index in &extra.sub_indices {
                collect(&sub_index.keys, &mut seen);
            }
        }

        let mut suggestions: Vec<String> = seen.into_iter().collect();

        suggestions.sort_unstable_by(|a, b| {
            let a_is_numeric = a.chars().all(|ch| ch.is_ascii_digit());
            let b_is_numeric = b.chars().all(|ch| ch.is_ascii_digit());
            let a_rank = (
                !a.starts_with(segment_prefix),
                segment_prefix.is_empty() && a_is_numeric,
                a.len(),
                a.as_str(),
            );
            let b_rank = (
                !b.starts_with(segment_prefix),
                segment_prefix.is_empty() && b_is_numeric,
                b.len(),
                b.as_str(),
            );
            a_rank.cmp(&b_rank)
        });

        suggestions
            .into_iter()
            .take(limit)
            .map(|candidate| format!("{base}{candidate}"))
            .collect()
    }
}

/// Esegue JSON escaping della stringa s nell'output buffer (no allocazioni intermedie).
fn json_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

// ── Lazy-span streaming search ────────────────────────────────────────────────

impl JsonIndex {
    /// Searches within a lazy node's raw bytes by streaming through the file.
    ///
    /// For each element in the lazy span, uses a fast byte pre-filter (`memmem`) to skip
    /// elements that cannot possibly match, then parses only the matching elements.
    /// Matching leaf nodes are materialized as sub-indices with correct path info.
    ///
    /// Returns global IDs of matching leaf nodes (usable with `get_path_any` etc.).
    pub fn search_in_lazy_node_with_options(
        &self,
        lazy_node_id: u32,
        query: &str,
        target: &str,
        case_sensitive: bool,
        use_regex: bool,
        exact_match: bool,
        max_results: usize,
        multiline: bool,
        dot_all: bool,
    ) -> Result<Vec<u32>, String> {
        let node = &self.nodes[lazy_node_id as usize];
        if !node.kind().is_lazy() || max_results == 0 {
            return Ok(vec![]);
        }

        let is_large_array = node.kind() == NodeKind::LazyArray && self.is_large_lazy(lazy_node_id);
        if !is_large_array {
            self.materialize_lazy_node(lazy_node_id)?;

            let extra = self.extra.lock().unwrap();
            let base = extra.base;
            let Some(mat) = extra.mat.get(&lazy_node_id) else {
                return Ok(Vec::new());
            };
            let sub_idx = mat.sub_idx;
            let sub_index = Arc::clone(&extra.sub_indices[sub_idx]);
            drop(extra);

            let matches = sub_index.search(
                query,
                target,
                case_sensitive,
                use_regex,
                exact_match,
                max_results,
                None,
                multiline,
                dot_all,
            );

            return Ok(matches
                .into_iter()
                .filter_map(|sub_id| {
                    if sub_id == sub_index.root && node.kind() == NodeKind::LazyObject {
                        None
                    } else {
                        Some(base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id)
                    }
                })
                .collect());
        }

        self.search_in_lazy_node_streaming(
            lazy_node_id,
            query,
            target,
            case_sensitive,
            use_regex,
            exact_match,
            max_results,
            multiline,
            dot_all,
        )
    }

    fn search_in_lazy_node_streaming(
        &self,
        lazy_node_id: u32,
        query: &str,
        target: &str,
        case_sensitive: bool,
        use_regex: bool,
        exact_match: bool,
        max_results: usize,
        multiline: bool,
        dot_all: bool,
    ) -> Result<Vec<u32>, String> {
        let node = &self.nodes[lazy_node_id as usize];
        if !node.kind().is_lazy() || max_results == 0 {
            return Ok(vec![]);
        }
        let span_id = node.value_data as usize;
        if span_id >= self.lazy_spans.len() {
            return Ok(vec![]);
        }
        let span = self.lazy_spans[span_id];

        let source = self.source_backing()?;
        let mmap = source.as_bytes();
        let span_start = span.file_offset as usize;
        let span_end = span_start + span.byte_len as usize;
        let bytes = &mmap[span_start..span_end.min(mmap.len())];

        let is_array = node.kind() == NodeKind::LazyArray;
        let query_lower = query.to_lowercase();
        // Byte sequence used for the fast pre-filter
        let needle: &[u8] = if case_sensitive {
            query.as_bytes()
        } else {
            query_lower.as_bytes()
        };

        let base = { self.extra.lock().unwrap().base };

        let mut results: Vec<u32> = Vec::new();
        let mut elem_index: usize = 0;
        let mut pos = 0usize;

        // Skip to opening '[' or '{'
        while pos < bytes.len() && !matches!(bytes[pos], b'[' | b'{') {
            pos += 1;
        }
        if pos >= bytes.len() {
            return Ok(results);
        }
        pos += 1;

        loop {
            // Skip whitespace and commas between elements
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b',') {
                pos += 1;
            }
            if pos >= bytes.len() || matches!(bytes[pos], b']' | b'}') {
                break;
            }

            let outer_start = pos;

            // For objects: skip the key part (advance pos to value start).
            // `outer_start` covers key+value for the pre-filter;
            // `value_start` covers only the value for JSON parsing.
            if !is_array && pos < bytes.len() && bytes[pos] == b'"' {
                pos = scan_json_string(bytes, pos);
                while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b':')
                {
                    pos += 1;
                }
            }
            let value_start = pos;

            pos = scan_json_value_end(bytes, pos);
            let elem_end = pos.min(bytes.len());

            // Pre-filter on full element bytes (key+value for objects) to avoid false negatives.
            let full_elem = &bytes[outer_start..elem_end];
            // Parse only the value bytes (valid JSON for both arrays and objects).
            let elem_bytes = &bytes[value_start..elem_end];

            // Fast byte pre-filter before parsing
            let maybe_matches = if use_regex {
                true
            } else if case_sensitive {
                memchr::memmem::find(full_elem, needle).is_some()
            } else {
                // Elements are typically small, so a lowercase copy is cheap and
                // avoids reparsing elements that clearly cannot match.
                let lower: Vec<u8> = full_elem.iter().map(|b| b.to_ascii_lowercase()).collect();
                memchr::memmem::find(&lower, needle).is_some()
            };

            if maybe_matches {
                if let Ok(sub_index) = JsonIndex::from_slice(elem_bytes) {
                    let matching_sub: Vec<u32> = sub_index.search(
                        query,
                        target,
                        case_sensitive,
                        use_regex,
                        exact_match,
                        max_results.saturating_sub(results.len()),
                        None,
                        multiline,
                        dot_all,
                    );

                    if !matching_sub.is_empty() {
                        let sub_index = Arc::new(sub_index);
                        let sub_root = sub_index.root;
                        let mut extra = self.extra.lock().unwrap();
                        let sub_idx = extra.sub_indices.len();
                        let root_global = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_root;

                        // Register parent → lazy node and key → element index
                        extra.extra_parent.insert(root_global, lazy_node_id);
                        extra
                            .extra_key_override
                            .insert(root_global, elem_index.to_string());

                        // Register all descendants so get_path_any works transitively
                        Self::register_sub_descendants(
                            &sub_index,
                            sub_root,
                            root_global,
                            sub_idx,
                            base,
                            &mut extra.extra_parent,
                        );

                        extra.sub_indices.push(sub_index);

                        for sub_id in matching_sub {
                            let global_id = base + (sub_idx as u32) * SUB_INDEX_ID_RANGE + sub_id;
                            results.push(global_id);
                            if results.len() >= max_results {
                                break;
                            }
                        }
                    }
                }
            }

            elem_index += 1;
            if results.len() >= max_results {
                break;
            }
        }

        Ok(results)
    }

    pub fn search_in_lazy_node(
        &self,
        lazy_node_id: u32,
        query: &str,
        case_sensitive: bool,
        exact_match: bool,
        max_results: usize,
    ) -> Result<Vec<u32>, String> {
        self.search_in_lazy_node_with_options(
            lazy_node_id,
            query,
            "both",
            case_sensitive,
            false,
            exact_match,
            max_results,
            false,
            false,
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(json: &str) -> JsonIndex {
        JsonIndex::from_str(json).expect("parse failed")
    }

    fn search_paths(
        index: &JsonIndex,
        query: &str,
        target: &str,
        case_sensitive: bool,
        use_regex: bool,
        exact_match: bool,
        max_results: usize,
        path: Option<&str>,
        multiline: bool,
        dot_all: bool,
    ) -> Vec<String> {
        index
            .search(
                query,
                target,
                case_sensitive,
                use_regex,
                exact_match,
                max_results,
                path,
                multiline,
                dot_all,
            )
            .into_iter()
            .map(|id| index.get_path(id))
            .collect()
    }

    // ── BFS correctness ───────────────────────────────────────────────────────

    #[test]
    fn object_children_have_unique_ids() {
        let index = idx(r#"{"a":1,"b":2,"c":3}"#);
        let root_children = index.get_children_slice(index.root);
        let ids: Vec<u32> = root_children.to_vec();
        let unique: std::collections::HashSet<u32> = ids.iter().cloned().collect();
        assert_eq!(ids.len(), unique.len(), "fratelli con lo stesso ID");
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn array_children_have_unique_ids() {
        let index = idx(r#"[10,20,30,40]"#);
        let root_children = index.get_children_slice(index.root);
        let ids: Vec<u32> = root_children.to_vec();
        let unique: std::collections::HashSet<u32> = ids.iter().cloned().collect();
        assert_eq!(ids.len(), unique.len(), "elementi array con lo stesso ID");
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn nested_object_children_unique_ids() {
        // verifica il bug BFS per nodi con più figli a più livelli
        let index = idx(r#"{"x":{"a":1,"b":2},"y":{"c":3,"d":4}}"#);
        let all_ids: Vec<u32> = (0..index.nodes.len() as u32).collect();
        let unique: std::collections::HashSet<u32> = all_ids.iter().cloned().collect();
        assert_eq!(all_ids.len(), unique.len());
    }

    #[test]
    fn root_has_correct_child_count() {
        let index = idx(r#"{"a":1,"b":2,"c":3,"d":4,"e":5}"#);
        assert_eq!(index.get_children_slice(index.root).len(), 5);
    }

    // ── get_path ──────────────────────────────────────────────────────────────

    #[test]
    fn path_root_object() {
        let index = idx(r#"{"name":"test"}"#);
        let name_id = index.get_children_slice(index.root)[0];
        assert_eq!(index.get_path(name_id), "$.name");
    }

    #[test]
    fn path_nested() {
        let index = idx(r#"{"user":{"age":30}}"#);
        let user_id = index.get_children_slice(index.root)[0];
        let age_id = index.get_children_slice(user_id)[0];
        assert_eq!(index.get_path(age_id), "$.user.age");
    }

    #[test]
    fn path_array_element() {
        let index = idx(r#"{"items":[1,2,3]}"#);
        let items_id = index.get_children_slice(index.root)[0];
        let second = index.get_children_slice(items_id)[1];
        assert_eq!(index.get_path(second), "$.items.1");
    }

    #[test]
    fn path_root_node_is_dollar() {
        let index = idx(r#"{"x":1}"#);
        assert_eq!(index.get_path(index.root), "$");
    }

    // ── build_raw (round-trip) ────────────────────────────────────────────────

    fn normalize(s: &str) -> sonic_rs::Value {
        sonic_rs::from_str(s).unwrap()
    }

    #[test]
    fn roundtrip_simple_object() {
        let src = r#"{"name":"Alice","age":30}"#;
        let index = idx(src);
        let raw = index.build_raw(index.root);
        assert_eq!(normalize(&raw), normalize(src));
    }

    #[test]
    fn roundtrip_nested() {
        let src = r#"{"a":{"b":{"c":42}}}"#;
        let index = idx(src);
        assert_eq!(normalize(&index.build_raw(index.root)), normalize(src));
    }

    #[test]
    fn roundtrip_array() {
        let src = r#"[1,2,3,"hello",true,null]"#;
        let index = idx(src);
        assert_eq!(normalize(&index.build_raw(index.root)), normalize(src));
    }

    #[test]
    fn roundtrip_empty_object() {
        let src = r#"{}"#;
        let index = idx(src);
        assert_eq!(index.build_raw(index.root), "{}");
    }

    #[test]
    fn roundtrip_empty_array() {
        let src = r#"[]"#;
        let index = idx(src);
        assert_eq!(index.build_raw(index.root), "[]");
    }

    #[test]
    fn roundtrip_string_escaping() {
        let src = r#"{"msg":"hello\nworld\t\"quoted\""}"#;
        let index = idx(src);
        assert_eq!(normalize(&index.build_raw(index.root)), normalize(src));
    }

    #[test]
    fn roundtrip_subtree() {
        let src = r#"{"outer":{"inner":[1,2,3]}}"#;
        let index = idx(src);
        let outer_id = index.get_children_slice(index.root)[0];
        let inner_id = index.get_children_slice(outer_id)[0];
        assert_eq!(normalize(&index.build_raw(inner_id)), normalize("[1,2,3]"));
    }

    // ── search ────────────────────────────────────────────────────────────────

    #[test]
    fn search_by_value() {
        let index = idx(r#"{"name":"Alice","city":"Rome"}"#);
        let results = search_paths(
            &index, "Alice", "values", false, false, false, 10, None, false, false,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "$.name");
    }

    #[test]
    fn search_by_key() {
        let index = idx(r#"{"username":"bob","email":"b@b.com"}"#);
        let results = search_paths(
            &index, "email", "keys", false, false, false, 10, None, false, false,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "$.email");
    }

    #[test]
    fn search_case_insensitive() {
        let index = idx(r#"{"msg":"Hello World"}"#);
        let results = index.search(
            "hello", "values", false, false, false, 10, None, false, false,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_case_sensitive_no_match() {
        let index = idx(r#"{"msg":"Hello World"}"#);
        let results = index.search(
            "hello", "values", true, false, false, 10, None, false, false,
        );
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn search_regex() {
        let index = idx(r#"{"a":"foo123","b":"bar456","c":"baz"}"#);
        let results = index.search(r"\d+", "values", false, true, false, 10, None, false, false);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_max_results() {
        let arr: String = (0..20)
            .map(|i| format!("\"item{}\"", i))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!("[{}]", arr);
        let index = idx(&json);
        let results = index.search("item", "values", false, false, false, 5, None, false, false);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn search_no_results() {
        let index = idx(r#"{"a":"hello"}"#);
        let results = index.search("xyz", "both", false, false, false, 10, None, false, false);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn search_both_keys_and_values() {
        let index = idx(r#"{"target":"other","other":"value"}"#);
        let results = index.search("other", "both", false, false, false, 10, None, false, false);
        // "other" appare come chiave di "other" e come valore di "target"
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_exact_match() {
        let index = idx(r#"{"a":"hello world","b":"hello","c":"say hello"}"#);
        let exact = search_paths(
            &index, "hello", "values", false, false, true, 10, None, false, false,
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0], "$.b");
        let partial = index.search(
            "hello", "values", false, false, false, 10, None, false, false,
        );
        assert_eq!(partial.len(), 3);
    }

    #[test]
    fn search_limited_to_path() {
        let index = idx(r#"{"users":[{"name":"Alice"},{"name":"Bob"}],"meta":{"name":"Catalog"}}"#);
        let results = search_paths(
            &index,
            "name",
            "keys",
            false,
            false,
            false,
            10,
            Some("$.users.0"),
            false,
            false,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "$.users.0.name");
    }

    #[test]
    fn search_invalid_path_returns_empty_results() {
        let index = idx(r#"{"users":[{"name":"Alice"}]}"#);
        let results = index.search(
            "Alice",
            "values",
            false,
            false,
            false,
            10,
            Some("$.missing"),
            false,
            false,
        );
        assert!(results.is_empty());
    }

    #[test]
    fn scalar_root_string() {
        let index = idx(r#""hello world""#);
        assert_eq!(index.nodes.len(), 1);
        assert_eq!(index.get_children_slice(index.root).len(), 0);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.kind(), NodeKind::Str));
    }

    #[test]
    fn scalar_root_number() {
        let index = idx("42");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.kind(), NodeKind::Num));
    }

    #[test]
    fn scalar_root_bool() {
        let index = idx("true");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.kind(), NodeKind::Bool));
        assert_eq!(root.value_data, 1);
    }

    #[test]
    fn scalar_root_null() {
        let index = idx("null");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.kind(), NodeKind::Null));
    }

    #[test]
    fn search_objects_by_single_property() {
        let index = idx(
            r#"{"items":[{"marketing_lingua":"Acciaio Anticato Lucido","title":"A"},{"marketing_lingua":"Ottone","title":"B"}]}"#,
        );
        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "marketing_lingua".to_string(),
                operator: ObjectSearchOperator::Contains,
                value: Some("Acciaio".to_string()),
                ..Default::default()
            }],
            false,
            false,
            10,
            None,
        );
        let paths: Vec<String> = results.into_iter().map(|id| index.get_path(id)).collect();
        assert_eq!(paths, vec!["$.items.0"]);
    }

    #[test]
    fn search_objects_matches_all_filters() {
        let index = idx(
            r#"{"items":[{"marketing_lingua":"Acciaio Anticato Lucido","finish":"Satinato"},{"marketing_lingua":"Acciaio Anticato Lucido","finish":"Lucido"}]}"#,
        );
        let results = index.search_objects(
            &[
                ObjectSearchFilter {
                    path: "marketing_lingua".to_string(),
                    operator: ObjectSearchOperator::Contains,
                    value: Some("Acciaio".to_string()),
                    ..Default::default()
                },
                ObjectSearchFilter {
                    path: "finish".to_string(),
                    operator: ObjectSearchOperator::Equals,
                    value: Some("Lucido".to_string()),
                    ..Default::default()
                },
            ],
            false,
            false,
            10,
            None,
        );
        let paths: Vec<String> = results.into_iter().map(|id| index.get_path(id)).collect();
        assert_eq!(paths, vec!["$.items.1"]);
    }

    #[test]
    fn search_objects_is_limited_to_scope_path() {
        let index =
            idx(r#"{"catalog":{"items":[{"code":"A1"}]},"archive":{"items":[{"code":"A1"}]}}"#);
        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "code".to_string(),
                operator: ObjectSearchOperator::Equals,
                value: Some("A1".to_string()),
                ..Default::default()
            }],
            true,
            true,
            10,
            Some("$.catalog.items"),
        );
        let paths: Vec<String> = results.into_iter().map(|id| index.get_path(id)).collect();
        assert_eq!(paths, vec!["$.catalog.items.0"]);
    }

    #[test]
    fn suggest_property_paths_uses_existing_keys() {
        let index = idx(r#"{"content":{"mainImage":{"url":"x"}},"marketing_lingua":"it"}"#);
        let suggestions = index.suggest_property_paths("content.ma", 5);
        assert!(suggestions.contains(&"content.mainImage".to_string()));
    }

    #[test]
    fn search_objects_can_match_keys_case_insensitively() {
        let index = idx(r#"{"items":[{"Marketing_Lingua":"Acciaio"}]}"#);
        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "marketing_lingua".to_string(),
                operator: ObjectSearchOperator::Equals,
                value: Some("Acciaio".to_string()),
                ..Default::default()
            }],
            false,
            true,
            10,
            None,
        );
        let paths: Vec<String> = results.into_iter().map(|id| index.get_path(id)).collect();
        assert_eq!(paths, vec!["$.items.0"]);
    }

    #[test]
    fn search_objects_supports_array_segments_in_paths() {
        let index = idx(
            r#"{"items":[{"content":{"mainImage":[{"url":"https://a.example"}]}},{"content":{"mainImage":[{"url":"https://b.example"}]}}]}"#,
        );
        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "content.mainImage.0.url".to_string(),
                operator: ObjectSearchOperator::Contains,
                value: Some("b.example".to_string()),
                ..Default::default()
            }],
            true,
            false,
            10,
            None,
        );
        let paths: Vec<String> = results.into_iter().map(|id| index.get_path(id)).collect();
        assert_eq!(paths, vec!["$.items.1"]);
    }

    #[test]
    fn suggest_property_paths_returns_initial_suggestions_on_empty_prefix() {
        let index = idx(r#"{"content":{"mainImage":{"url":"x"}},"marketing_lingua":"it"}"#);
        let suggestions = index.suggest_property_paths("", 5);
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"content".to_string()));
    }

    #[test]
    fn suggest_property_paths_includes_keys_from_materialized_lazy_nodes() {
        // Regressione: con always-lazy loading le chiavi degli oggetti dentro i
        // lazy span non finivano in self.keys → la tendina mostrava solo le chiavi
        // del top-level e non quelle dei nodi figli materializzati.
        let json =
            r#"[{"name":"Alice","age":30,"role":"admin"},{"name":"Bob","age":25,"role":"user"}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        // Materializza il primo figlio del nodo lazy (simula expand-all)
        let _ = index.get_children_any(lazy_id);

        let suggestions = index.suggest_property_paths("", 20);
        // Dopo materializzazione, le chiavi degli oggetti figli devono comparire
        assert!(
            suggestions.contains(&"name".to_string()),
            "manca 'name': {:?}",
            suggestions
        );
        assert!(
            suggestions.contains(&"age".to_string()),
            "manca 'age': {:?}",
            suggestions
        );
        assert!(
            suggestions.contains(&"role".to_string()),
            "manca 'role': {:?}",
            suggestions
        );
    }

    // ── Regressione: search_objects su file lazy non trova nulla ──────────────
    //
    // Con always-lazy loading self.nodes ha pochissimi nodi (solo la struttura
    // top-level). Gli oggetti reali stanno nei sub-index materializzati.
    // Prima: search_objects scansionava solo self.nodes → zero risultati.

    #[test]
    fn search_objects_finds_results_in_lazy_loaded_files() {
        let json = r#"[{"name":"Alice","age":30,"role":"admin"},{"name":"Bob","age":25,"role":"user"},{"name":"Carol","age":35,"role":"admin"}]"#;
        let (index, _, _tmp) = make_lazy_index_from_json(json);

        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "role".to_string(),
                operator: ObjectSearchOperator::Equals,
                value: Some("admin".to_string()),
                ..Default::default()
            }],
            false,
            false,
            10,
            None,
        );
        assert_eq!(
            results.len(),
            2,
            "dovrebbe trovare Alice e Carol: {:?}",
            results
        );
    }

    #[test]
    fn search_objects_finds_by_multiple_filters_in_lazy_file() {
        let json = r#"[{"name":"Alice","active":true},{"name":"Bob","active":false},{"name":"Carol","active":true}]"#;
        let (index, _, _tmp) = make_lazy_index_from_json(json);

        let results = index.search_objects(
            &[
                ObjectSearchFilter {
                    path: "name".to_string(),
                    operator: ObjectSearchOperator::Contains,
                    value: Some("a".to_string()),
                    ..Default::default()
                },
                ObjectSearchFilter {
                    path: "active".to_string(),
                    operator: ObjectSearchOperator::Exists,
                    value: None,
                    ..Default::default()
                },
            ],
            false,
            false,
            10,
            None,
        );
        // "Alice" (name contains 'a') e "Carol" (name contains 'a')
        assert!(
            results.len() >= 2,
            "attesi almeno 2 risultati: {:?}",
            results
        );
    }

    // ── get_expanded_slice / expanded_visible_count con lazy loading ──────────
    //
    // Con always-lazy loading i nodi reali stanno nei sub-indices materializzati.
    // Prima di questi fix, get_expanded_slice restituiva solo i nodi del main index
    // (1-2 nodi lazy) e expanded_visible_count restituiva 1.

    #[test]
    fn get_expanded_slice_includes_materialized_sub_index_nodes() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let json = r#"[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"},{"id":3,"name":"Carol"}]"#;
        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");

        // Prima della materializzazione: solo i lazy nodes del main index
        let count_before = index.expanded_visible_count();
        // Materializziamo tutti i lazy nodes al top level
        let root_children = index.get_children_slice(index.root).to_vec();
        for &child_id in &root_children {
            if index.nodes[child_id as usize].kind().is_lazy() {
                let _ = index.get_children_any(child_id);
            }
        }

        // Dopo materializzazione: get_expanded_slice deve includere i nodi dei sub-index
        let slice = index.get_expanded_slice(0, 1000);
        let count_after = index.expanded_visible_count();

        // Devono esserci almeno i 3 oggetti top-level + i loro campi (id, name × 3)
        assert!(
            slice.len() >= 3 + 6,
            "attesi ≥9 nodi nella slice, trovati {}: {:?}",
            slice.len(),
            slice.iter().map(|r| r.id).collect::<Vec<_>>()
        );
        assert!(
            count_after > count_before,
            "expanded_visible_count deve crescere dopo materializzazione: {} → {}",
            count_before,
            count_after
        );
        assert_eq!(
            count_after,
            slice.len(),
            "expanded_visible_count deve coincidere con slice.len() dopo materializzazione"
        );
    }

    #[test]
    fn get_expanded_slice_paginate_works_with_lazy_nodes() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // 10 oggetti con 2 campi ciascuno → 10 lazy obj + 20 sub-fields = 30 nodi visibili
        let mut json = String::from("[");
        for i in 0..10 {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(r#"{{"id":{i},"val":{}}}"#, i * 10));
        }
        json.push(']');

        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        // Materializza tutti i lazy nodes
        let root_children = index.get_children_slice(index.root).to_vec();
        for &child_id in &root_children {
            if index.nodes[child_id as usize].kind().is_lazy() {
                let _ = index.get_children_any(child_id);
            }
        }

        let all = index.get_expanded_slice(0, 1000);
        let total = all.len();
        assert!(total >= 10, "attesi ≥10 nodi, trovati {total}");

        // La paginazione deve restituire lo stesso risultato in ordine
        let mut paginated: Vec<VisibleSliceRow> = Vec::new();
        let page_size = 7;
        let mut offset = 0;
        loop {
            let page = index.get_expanded_slice(offset, page_size);
            if page.is_empty() {
                break;
            }
            offset += page.len();
            paginated.extend(page);
            if offset >= total {
                break;
            }
        }

        assert_eq!(
            all.iter().map(|r| r.id).collect::<Vec<_>>(),
            paginated.iter().map(|r| r.id).collect::<Vec<_>>(),
            "la paginazione deve restituire gli stessi nodi nell'ordine corretto"
        );
    }

    #[test]
    fn node_id_is_preorder_index() {
        // Il parser streaming alloca in DFS preorder: node_id coincide con l'ordine di visita.
        let index = idx(r#"{"a":{"x":1},"b":2}"#);
        let a_id = index.get_children_slice(index.root)[0];
        let b_id = index.get_children_slice(index.root)[1];
        let x_id = index.get_children_slice(a_id)[0];
        // root < a < x < b in DFS preorder
        assert!(a_id < x_id && x_id < b_id);
    }

    // ── node values ───────────────────────────────────────────────────────────

    #[test]
    fn node_types_correct() {
        let index = idx(r#"{"s":"hello","n":42,"b":true,"null":null,"arr":[],"obj":{}}"#);
        let children = index.get_children_slice(index.root);
        let type_of = |id: u32| match index.nodes[id as usize].kind() {
            NodeKind::Str => "string",
            NodeKind::Num => "number",
            NodeKind::Bool => "bool",
            NodeKind::Null => "null",
            NodeKind::Array => "array",
            NodeKind::Object => "object",
            NodeKind::LazyObject => "lazy-object",
            NodeKind::LazyArray => "lazy-array",
        };
        let keys: Vec<&str> = children
            .iter()
            .map(|&id| {
                let k = index.nodes[id as usize].string_key_id().unwrap();
                index.keys.get(k)
            })
            .collect();
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "s").unwrap()]),
            "string"
        );
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "n").unwrap()]),
            "number"
        );
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "b").unwrap()]),
            "bool"
        );
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "null").unwrap()]),
            "null"
        );
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "arr").unwrap()]),
            "array"
        );
        assert_eq!(
            type_of(children[keys.iter().position(|&k| k == "obj").unwrap()]),
            "object"
        );
    }

    #[test]
    fn string_pool_deduplicates_keys() {
        let index = idx(r#"[{"name":"a"},{"name":"b"},{"name":"c"}]"#);
        // "name" deve essere internato una volta sola
        let name_count = (0..index.keys.len() as u32)
            .filter(|&id| index.keys.get(id) == "name")
            .count();
        assert_eq!(name_count, 1);
    }

    #[test]
    fn array_indexes_are_not_interned_as_keys() {
        let index = idx(r#"["a","b","c"]"#);
        assert!(index.keys.id_of("0").is_none());
        assert!(index.keys.id_of("1").is_none());
        assert!(index.keys.id_of("2").is_none());
        assert_eq!(
            index.get_path(index.get_children_slice(index.root)[1]),
            "$.1"
        );
    }

    #[test]
    fn expanded_visible_count_excludes_synthetic_root() {
        let index = idx(r#"{"a":{"b":1},"c":[2,3]}"#);
        assert_eq!(index.expanded_visible_count(), index.nodes.len() - 1);
    }

    #[test]
    fn expanded_slice_returns_preorder_rows() {
        let index = idx(r#"{"a":{"b":1},"c":[2,3]}"#);
        let rows = index.get_expanded_slice(0, 10);
        let paths: Vec<String> = rows.iter().map(|row| index.get_path(row.id)).collect();
        let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
        assert_eq!(paths, vec!["$.a", "$.a.b", "$.c", "$.c.0", "$.c.1"]);
        assert_eq!(depths, vec![0, 1, 0, 1, 1]);
    }

    #[test]
    fn expanded_slice_supports_offsets_inside_subtrees() {
        let index = idx(r#"{"a":{"b":1},"c":[2,3]}"#);
        let rows = index.get_expanded_slice(2, 2);
        let paths: Vec<String> = rows.iter().map(|row| index.get_path(row.id)).collect();
        assert_eq!(paths, vec!["$.c", "$.c.0"]);
    }

    /// Verifica che children_len e subtree_len siano corretti dopo lo split ContainerMeta.
    /// Questo test cattura regressioni introdotte da modifiche all'indice.
    #[test]
    fn container_meta_split_correctness() {
        // Array flat con 200 oggetti {id, name} → 1 + 200*3 = 601 nodi
        let mut s = String::from("[");
        for i in 0..200usize {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(r#"{{"id":{},"name":"item{}"}}"#, i, i));
        }
        s.push(']');
        let index = idx(&s);

        // La radice è un array con 200 figli
        assert_eq!(
            index.children_len(index.root),
            200,
            "root children_len wrong"
        );
        assert_eq!(index.subtree_len(index.root), 600, "root subtree_len wrong");

        // Ogni item è un oggetto con 2 figli (id, name)
        let mut count = 0usize;
        for child_id in index.children_iter(index.root) {
            assert_eq!(
                index.children_len(child_id),
                2,
                "item children_len wrong for id={}",
                child_id
            );
            assert_eq!(
                index.subtree_len(child_id),
                2,
                "item subtree_len wrong for id={}",
                child_id
            );
            count += 1;
        }
        assert_eq!(count, 200, "children_iter should yield exactly 200 items");

        // Struttura annidata: {users: [{name, age}, ...]}
        let mut json2 = String::from(r#"{"users":["#);
        for i in 0..50usize {
            if i > 0 {
                json2.push(',');
            }
            json2.push_str(&format!(r#"{{"name":"user{}","age":{}}}"#, i, i + 20));
        }
        json2.push_str("]}");
        let idx2 = idx(&json2);
        // root → object {users: array}
        assert_eq!(idx2.children_len(idx2.root), 1);
        let users_id = idx2.children_iter(idx2.root).next().unwrap();
        // users → array con 50 elementi
        assert_eq!(idx2.children_len(users_id), 50);
        // subtree di users = 50 oggetti × 3 nodi ciascuno = 150
        assert_eq!(idx2.subtree_len(users_id), 150);
    }

    // ── is_large_lazy ────────────────────────────────────────────────────────

    #[test]
    fn is_large_lazy_false_for_normal_nodes() {
        // from_str non crea mai lazy node (usa solo from_file)
        let index = idx(r#"{"data":[1,2,3]}"#);
        for id in 0..index.nodes.len() as u32 {
            assert!(
                !index.is_large_lazy(id),
                "nessun nodo deve essere large-lazy in un indice da stringa"
            );
        }
    }

    // ── children_count_any ───────────────────────────────────────────────────

    #[test]
    fn children_count_any_matches_children_len_for_normal_nodes() {
        let index = idx(r#"{"a":1,"b":2,"c":3}"#);
        // Per nodi normali i due metodi devono concordare
        for id in 0..index.nodes.len() as u32 {
            assert_eq!(
                index.children_count_any(id),
                index.children_len(id),
                "children_count_any != children_len per id={id}"
            );
        }
    }

    #[test]
    fn children_count_any_zero_for_leaf_nodes() {
        let index = idx(r#"{"s":"hello","n":42,"b":true,"null":null}"#);
        let leaves: Vec<u32> = index.get_children_slice(index.root).to_vec();
        for id in leaves {
            assert_eq!(
                index.children_count_any(id),
                0,
                "foglie devono avere count 0"
            );
        }
    }

    // ── get_children_any for normal nodes ────────────────────────────────────

    #[test]
    fn get_children_any_returns_same_as_children_iter_for_objects() {
        let index = idx(r#"{"x":1,"y":2,"z":3}"#);
        let via_iter: Vec<u32> = index.children_iter(index.root).collect();
        let via_any = index
            .get_children_any(index.root)
            .expect("get_children_any failed");
        assert_eq!(via_iter, via_any);
    }

    #[test]
    fn get_children_any_returns_same_as_children_iter_for_arrays() {
        let index = idx(r#"[10,20,30,40,50]"#);
        let via_iter: Vec<u32> = index.children_iter(index.root).collect();
        let via_any = index
            .get_children_any(index.root)
            .expect("get_children_any failed");
        assert_eq!(via_iter, via_any);
    }

    #[test]
    fn get_children_any_empty_for_leaf() {
        let index = idx(r#""hello""#);
        let result = index
            .get_children_any(index.root)
            .expect("get_children_any failed");
        assert!(result.is_empty());
    }

    // ── search_in_lazy_node (streaming search) ────────────────────────────────
    //
    // `from_str` non crea lazy node (solo `from_file`), quindi per
    // testare search_in_lazy_node usiamo un file temporaneo e `from_file`.
    // Il file è abbastanza piccolo da essere parsato eagerly, ma il metodo
    // accetta qualsiasi nodo lazy — lo creiamo manualmente iniettandone uno.

    /// Crea un JsonIndex con un lazy span artificiale che punta a un file temp.
    /// Utile per testare `search_in_lazy_node` indipendentemente dalla soglia 512MB.
    fn make_lazy_index_from_json(json: &str) -> (JsonIndex, u32, tempfile::NamedTempFile) {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Scrive il JSON in un file temp
        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        // Crea un indice "vuoto" con source_file impostato
        // e inietta manualmente un nodo LazyArray che copre l'intero file.
        let mut index = idx("[]"); // indice base minimo
        index.source_file = Some(tmp.path().to_str().unwrap().to_string());

        // Aggiungi lo span che copre tutto il file
        let file_len = json.len() as u64;
        index.lazy_spans.push(LazySpan {
            file_offset: 0,
            byte_len: file_len,
        });
        index.lazy_child_counts.push(1);

        // Trasforma il root node in LazyArray con ktype corretto (NO_KEY nelle bits basse)
        let span_idx = (index.lazy_spans.len() - 1) as u32;
        index.nodes[index.root as usize].ktype = Node::make_ktype(NodeKind::LazyArray, None);
        index.nodes[index.root as usize].value_data = span_idx;

        let root_id = index.root;
        (index, root_id, tmp)
    }

    #[test]
    fn search_in_lazy_node_finds_matching_elements() {
        let json =
            r#"[{"name":"Alice","age":30},{"name":"Bob","age":25},{"name":"Charlie","age":30}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let results = index
            .search_in_lazy_node(lazy_id, "Alice", true, false, 10)
            .expect("search failed");
        assert_eq!(results.len(), 1, "deve trovare solo Alice");
        let path = index.get_path_any(results[0]);
        assert!(
            path.contains("0"),
            "path deve contenere l'indice 0: {}",
            path
        );
    }

    #[test]
    fn search_in_lazy_node_case_insensitive() {
        let json = r#"[{"city":"Rome"},{"city":"ROME"},{"city":"Milan"}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let results = index
            .search_in_lazy_node(lazy_id, "rome", false, false, 10)
            .expect("search failed");
        assert_eq!(
            results.len(),
            2,
            "deve trovare Rome e ROME case-insensitive"
        );
    }

    #[test]
    fn materialize_lazy_node_preserves_nested_lazy_containers() {
        let json = r#"[{"name":"Alice","info":{"city":"Rome"},"tags":[1,2,3]}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        index
            .materialize_lazy_node(lazy_id)
            .expect("materialization failed");

        let rows = index
            .get_children_any(lazy_id)
            .expect("children of lazy root should load");
        assert_eq!(rows.len(), 1);
        assert_eq!(index.node_kind_any(rows[0]), NodeKind::Object);

        let fields = index
            .get_children_any(rows[0])
            .expect("children of row object should load");
        let mut seen = std::collections::HashMap::new();
        for field_id in fields {
            seen.insert(
                index.key_string_any(field_id).unwrap_or_default(),
                index.node_kind_any(field_id),
            );
        }

        assert_eq!(seen.get("name"), Some(&NodeKind::Str));
        assert_eq!(seen.get("info"), Some(&NodeKind::LazyObject));
        assert_eq!(seen.get("tags"), Some(&NodeKind::LazyArray));
    }

    #[test]
    fn search_objects_in_lazy_node_works_with_nested_lazy_fields_after_materialize() {
        let json = r#"[{"content":{"mainImage":[{"url":"https://cdn.example.com/images/0001.jpg"}]}}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let results = index
            .search_objects_in_lazy_node(
                lazy_id,
                &[ObjectSearchFilter {
                    path: "content.mainImage.0.url".to_string(),
                    operator: ObjectSearchOperator::Contains,
                    value: Some("cdn.example.com/images/".to_string()),
                    ..Default::default()
                }],
                true,
                false,
                10,
            )
            .expect("object search should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(index.get_path_any(results[0]), "$.0");
    }

    #[test]
    fn search_in_lazy_node_returns_empty_when_no_match() {
        let json = r#"[{"a":"foo"},{"a":"bar"}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let results = index
            .search_in_lazy_node(lazy_id, "xyz", true, false, 10)
            .expect("search failed");
        assert!(results.is_empty());
    }

    #[test]
    fn search_in_lazy_node_respects_max_results() {
        let items: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"v":"match{}"}}"#, i))
            .collect();
        let json = format!("[{}]", items.join(","));
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(&json);

        let results = index
            .search_in_lazy_node(lazy_id, "match", true, false, 5)
            .expect("search failed");
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn search_in_lazy_node_path_contains_element_index() {
        let json = r#"[{"x":1},{"x":2},{"target":"found"},{"x":4}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let results = index
            .search_in_lazy_node(lazy_id, "found", true, false, 10)
            .expect("search failed");
        assert_eq!(results.len(), 1);
        let path = index.get_path_any(results[0]);
        // Path should encode element index 2 (third element, 0-based)
        assert!(
            path.contains("2"),
            "path dovrebbe contenere indice '2': {}",
            path
        );
    }

    #[test]
    fn search_in_lazy_object_finds_values_not_just_keys() {
        // Bug: per nodi LazyObject, elem_bytes includeva "chiave": valore,
        // quindi from_slice parsava solo la chiave e ignorava il valore.
        // Ora elem_bytes contiene solo il valore → i match nei valori vengono trovati.
        let _json = r#"{"state":"ARIZ","region":"West","capital":"Phoenix"}"#;
        // Usa un LazyArray che contiene un oggetto (per testare il caso is_array=false
        // nel ciclo interno dell'oggetto che stiamo scansionando).
        // In realtà search_in_lazy_node è chiamato su un nodo lazy il cui span è un
        // oggetto o array — usiamo un array di oggetti per il test standard.
        let json_arr = r#"[{"state":"ARIZ","region":"West"},{"state":"CA","region":"West"}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json_arr);

        let results = index
            .search_in_lazy_node(lazy_id, "ARIZ", true, false, 10)
            .expect("search failed");
        assert_eq!(results.len(), 1, "deve trovare 1 risultato per ARIZ");
    }

    #[test]
    fn search_in_lazy_object_node_finds_values() {
        // Testa che i valori dentro un LazyObject siano cercati correttamente.
        // Prima del fix, from_slice riceveva '"state": "ARIZ"' (key+value),
        // parsava solo la stringa "state" e non trovava "ARIZ".
        let json = r#"{"us":{"state":"ARIZ"},"ca":{"state":"BC"}}"#;
        let (mut index, lazy_id, tmp) = make_lazy_index_from_json(json);
        // Il nodo lazy è un LazyArray che wrappa l'intero JSON come array.
        // Per testare LazyObject usiamo direttamente un oggetto come span.
        // Modifica il nodo root in LazyObject per simulare un oggetto lazy.
        index.nodes[lazy_id as usize].ktype = Node::make_ktype(NodeKind::LazyObject, None);

        let results = index
            .search_in_lazy_node(lazy_id, "ARIZ", true, false, 10)
            .expect("search failed");
        // Con il fix, trova "ARIZ" nel valore dell'elemento "us"
        assert!(
            !results.is_empty(),
            "deve trovare ARIZ nel valore di un LazyObject"
        );
        let _ = tmp; // mantieni il file temp vivo
    }

    #[test]
    fn resolve_path_any_descends_into_lazy_object() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let json = r#"{"settings":{"theme":"dark","lang":"en"}}"#;
        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let theme_id = index
            .resolve_path_any("$.settings.theme")
            .expect("path inside lazy object should resolve");

        assert_eq!(index.get_path_any(theme_id), "$.settings.theme");
    }

    #[test]
    fn search_in_lazy_node_with_options_finds_top_level_keys_in_lazy_object() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let json = r#"{"settings":{"theme":"dark","lang":"en"}}"#;
        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let settings_id = index
            .resolve_path_any("$.settings")
            .expect("settings path should resolve");

        let results = index
            .search_in_lazy_node_with_options(
                settings_id,
                "theme",
                "keys",
                true,
                false,
                false,
                10,
                false,
                false,
            )
            .expect("lazy key search failed");

        assert_eq!(results.len(), 1);
        assert_eq!(index.get_path_any(results[0]), "$.settings.theme");
    }

    #[test]
    fn search_objects_returns_canonical_main_id_for_lazy_object_root() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let json = r#"{"settings":{"theme":"dark","lang":"en"},"other":{"theme":"light"}}"#;
        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let settings_id = index
            .resolve_path_any("$.settings")
            .expect("settings path should resolve");

        let results = index.search_objects(
            &[ObjectSearchFilter {
                path: "theme".to_string(),
                operator: ObjectSearchOperator::Equals,
                value: Some("dark".to_string()),
                ..Default::default()
            }],
            false,
            false,
            10,
            None,
        );

        assert_eq!(results, vec![settings_id]);
        assert_eq!(index.get_path_any(results[0]), "$.settings");
    }

    #[test]
    fn search_objects_in_large_lazy_array_scans_beyond_first_page() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let users = (0..1505)
            .map(|idx| {
                let role = if idx == 1500 { "admin" } else { "user" };
                format!(r#"{{"role":"{}","name":"user-{}"}}"#, role, idx)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{"users":[{}]}}"#, users);

        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let mut index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let users_id = index
            .resolve_path_any("$.users")
            .expect("users path should resolve");
        let span_id = index.nodes[users_id as usize].value_data as usize;
        index.lazy_spans[span_id].byte_len = LAZY_PAGINATE_THRESHOLD;

        let results = index
            .search_objects_in_lazy_node(
                users_id,
                &[ObjectSearchFilter {
                    path: "role".to_string(),
                    operator: ObjectSearchOperator::Equals,
                    value: Some("admin".to_string()),
                    ..Default::default()
                }],
                false,
                false,
                10,
            )
            .expect("streaming object search failed");

        assert_eq!(results.len(), 1);
        assert_eq!(index.get_path_any(results[0]), "$.users.1500");
    }

    #[test]
    fn search_in_large_lazy_array_supports_regex_beyond_first_page() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let users = (0..1505)
            .map(|idx| format!(r#"{{"name":"user-{}"}}"#, idx))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{"users":[{}]}}"#, users);

        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let mut index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let users_id = index
            .resolve_path_any("$.users")
            .expect("users path should resolve");
        let span_id = index.nodes[users_id as usize].value_data as usize;
        index.lazy_spans[span_id].byte_len = LAZY_PAGINATE_THRESHOLD;

        let results = index
            .search_in_lazy_node_with_options(
                users_id,
                "^user-1500$",
                "values",
                true,
                true,
                false,
                10,
                false,
                false,
            )
            .expect("streaming regex search failed");

        assert_eq!(results.len(), 1);
        assert_eq!(index.get_path_any(results[0]), "$.users.1500.name");
    }

    #[test]
    fn get_lazy_children_page_reuses_cached_page_ids() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let users = (0..1505)
            .map(|idx| format!(r#"{{"name":"user-{}"}}"#, idx))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{"users":[{}]}}"#, users);

        let mut tmp = NamedTempFile::new().expect("tmp file");
        tmp.write_all(json.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let mut index = JsonIndex::from_file(tmp.path().to_str().unwrap()).expect("parse failed");
        let users_id = index
            .resolve_path_any("$.users")
            .expect("users path should resolve");
        let span_id = index.nodes[users_id as usize].value_data as usize;
        index.lazy_spans[span_id].byte_len = LAZY_PAGINATE_THRESHOLD;

        let first = index
            .get_lazy_children_page(users_id, 1000, 1000)
            .expect("first page load failed");
        let sub_indices_after_first = index.extra.lock().unwrap().sub_indices.len();
        let second = index
            .get_lazy_children_page(users_id, 1000, 1000)
            .expect("second page load failed");
        let extra = index.extra.lock().unwrap();

        assert_eq!(first, second);
        assert_eq!(extra.sub_indices.len(), sub_indices_after_first);
        assert!(extra.page_cache.contains_key(&(users_id, 1000, 1000)));
    }

    // ── Regressione: expand_subtree su nodi lazy non deve crashare ─────────────
    //
    // Bug: `has_children(extra_id)` accedeva a `self.nodes[extra_id as usize]`
    // senza verificare che extra_id < nodes.len(). I figli di nodi lazy
    // materializzati hanno ID extra (>= base) molto grandi → out-of-bounds.

    #[test]
    fn get_children_any_recursive_on_lazy_node_does_not_panic() {
        // Simula expand-all: chiama get_children_any ricorsivamente
        // partendo da un nodo lazy materializzato.
        // Prima era un crash (abort) in produzione.
        let json = r#"[{"a":1,"b":2},{"c":3},{"d":{"nested":true}}]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        // Prima espansione: figli del nodo lazy (extra IDs)
        let children = index
            .get_children_any(lazy_id)
            .expect("prima espansione fallita");
        assert!(!children.is_empty(), "il nodo lazy deve avere figli");

        // Seconda espansione: figli degli extra node (simula BFS di expand_subtree)
        for child_id in &children {
            let grandchildren = index
                .get_children_any(*child_id)
                .expect("espansione extra node fallita");
            // Per ogni nipote, verifica che get_children_any non panichi
            for gc_id in &grandchildren {
                let _ = index.get_children_any(*gc_id);
            }
        }
    }

    #[test]
    fn get_children_any_on_extra_leaf_returns_empty() {
        // I figli scalari di un nodo lazy materializzato devono restituire []
        // senza panic (prima crashava via has_children → nodes[extra_id]).
        let json = r#"[1, "hello", true, null]"#;
        let (index, lazy_id, _tmp) = make_lazy_index_from_json(json);

        let children = index
            .get_children_any(lazy_id)
            .expect("espansione lazy fallita");
        assert_eq!(children.len(), 4);

        for child_id in children {
            let grandchildren = index
                .get_children_any(child_id)
                .expect("get_children_any su foglia extra non deve fallire");
            assert!(
                grandchildren.is_empty(),
                "scalare non ha figli: id={}",
                child_id
            );
        }
    }

    // ── scan_top_level_entries: large entries must not stop the scan ──────────

    #[test]
    fn large_top_level_entry_followed_by_more_entries_all_indexed() {
        use std::io::Write;
        // Build an array where the first element's JSON value exceeds the fast-scan
        // budget (FAST_SCAN_SMALL_VALUE_THRESHOLD). Before the fix, the scanner
        // would break after this entry and miss subsequent ones.
        let large_str = "x".repeat(FAST_SCAN_SMALL_VALUE_THRESHOLD + 256);
        let json = format!("[{{\"data\": \"{large_str}\"}}, {{\"id\": 42}}, 99]");

        let path = std::env::temp_dir().join("jgtest_large_entry_regression.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(json.as_bytes()).unwrap();
        }
        let index = JsonIndex::from_file(path.to_str().unwrap()).expect("from_file must succeed");
        std::fs::remove_file(&path).ok();

        // Root array must contain all 3 entries, not just the first large one.
        assert_eq!(
            index.children_len(index.root),
            3,
            "tutti e 3 gli elementi devono essere indicizzati"
        );
    }
}

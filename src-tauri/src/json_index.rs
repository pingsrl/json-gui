use memchr::memmem;
use rayon::prelude::*;
use regex::Regex;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::fmt::Write as _;
use std::fs::File;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::marker::PhantomData;
use std::ptr::NonNull;

const LARGE_FILE_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024;
const VERY_LARGE_FILE_THRESHOLD_BYTES: u64 = 512 * 1024 * 1024;
const NODE_CAP_DIVISOR_DEFAULT: u64 = 50;
const NODE_CAP_DIVISOR_LARGE_FILE: u64 = 55;
const NODE_CAP_DIVISOR_VERY_LARGE_FILE: u64 = 80;
const STRING_BYTES_DIVISOR_DEFAULT: u64 = 10;
const STRING_BYTES_DIVISOR_LARGE_FILE: u64 = 20;
const STRING_BYTES_DIVISOR_VERY_LARGE_FILE: u64 = 40;
/// For files > VERY_LARGE_FILE_THRESHOLD_BYTES, string values longer than this
/// are stored truncated in val_strings.  This caps memory used by long strings
/// (descriptions, URLs, base64 blobs) while keeping the UI display correct
/// (previews are already limited to 80 chars).  The raw export of truncated
/// nodes will be shortened.
const VERY_LARGE_FILE_STR_MAX_BYTES: usize = 256;


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

fn capacity_hints(file_size: u64) -> (usize, usize) {
    let (node_divisor, string_divisor) = if file_size >= VERY_LARGE_FILE_THRESHOLD_BYTES {
        (
            NODE_CAP_DIVISOR_VERY_LARGE_FILE,
            STRING_BYTES_DIVISOR_VERY_LARGE_FILE,
        )
    } else if file_size >= LARGE_FILE_THRESHOLD_BYTES {
        (NODE_CAP_DIVISOR_LARGE_FILE, STRING_BYTES_DIVISOR_LARGE_FILE)
    } else {
        (NODE_CAP_DIVISOR_DEFAULT, STRING_BYTES_DIVISOR_DEFAULT)
    };
    // For very large files, cap node_cap conservatively – we prefer one
    // Vec doubling over pre-allocating hundreds of MB that may never be used.
    let node_cap = (file_size / node_divisor).min(80_000_000) as usize;
    let str_bytes = (file_size / string_divisor).min(200_000_000) as usize;
    (node_cap.max(1024), str_bytes.max(4096))
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
}

impl NodeKind {
    #[inline]
    pub fn is_container(self) -> bool {
        matches!(self, NodeKind::Object | NodeKind::Array)
    }
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
    container_children: Vec<u16>,
    keys: InternedStrings,
    val_strings: InternedStrings,
    nums_pool: Vec<f64>, // f64 pool: NodeKind::Num → nums_pool[value_data]
    /// Maximum bytes to store per string value (0 = unlimited).
    /// Set to VERY_LARGE_FILE_STR_MAX_BYTES for files over the threshold.
    str_max_bytes: usize,
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
        unsafe { f(self.ptr.as_ptr().as_mut().expect("stream ctx pointer is null")) }
    }
}

struct ValSeed {
    ctx: StreamCtxPtr<'static>,
    parent: u32,
    key: Option<NodeKey>,
}

impl<'de> DeserializeSeed<'de> for ValSeed {
    type Value = u32;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<u32, D::Error> {
        de.deserialize_any(ValVisitor {
            ctx: self.ctx,
            parent: self.parent,
            key: self.key,
        })
    }
}

struct ValVisitor {
    ctx: StreamCtxPtr<'static>,
    parent: u32,
    key: Option<NodeKey>,
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
        while let Some(key_str) = map.next_key::<Cow<'de, str>>()? {
            let kid = self.ctx.with_mut(|ctx| ctx.keys.intern(&key_str));
            map.next_value_seed(ValSeed {
                ctx: self.ctx,
                parent: id,
                key: Some(NodeKey::String(kid)),
            })?;
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
        let mut index = 0usize;
        loop {
            if index >= KEY_DATA_MASK as usize {
                return Err(de::Error::custom(
                    "array index exceeds inline storage capacity",
                ));
            }
            if seq
                .next_element_seed(ValSeed {
                    ctx: self.ctx,
                    parent: id,
                    key: Some(NodeKey::ArrayIndex(index as u32)),
                })?
                .is_none()
            {
                break;
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
    }
}

fn parse_streaming<'de, D: de::Deserializer<'de>>(de: D) -> Result<JsonIndex, D::Error> {
    parse_streaming_with_cap(de, 0, 0, 0)
}

fn parse_streaming_with_cap<'de, D: de::Deserializer<'de>>(
    de: D,
    node_cap: usize,
    str_bytes: usize,
    str_max_bytes: usize,
) -> Result<JsonIndex, D::Error> {
    let mut ctx = StreamCtx::with_capacity(node_cap, str_bytes);
    ctx.str_max_bytes = str_max_bytes;
    let ctx_ptr = StreamCtxPtr::new(&mut ctx);
    let ctx_ptr: StreamCtxPtr<'static> = unsafe { std::mem::transmute(ctx_ptr) };
    de.deserialize_any(ValVisitor {
        ctx: ctx_ptr,
        parent: u32::MAX,
        key: None,
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
    pub container_children: Vec<u16>, // children_len per container (capped at 65535)
    pub keys: InternedStrings,
    pub val_strings: InternedStrings, // compact interned string-value pool
    pub nums_pool: Vec<f64>,          // numeric values: NodeKind::Num(idx) → nums_pool[idx]
    pub root: u32,
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

// ---- Parallel parse helpers ----

/// Advances past ASCII whitespace; returns `None` if end of input.
fn skip_ws(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    if i < bytes.len() { Some(i) } else { None }
}

/// Skips a JSON string starting AFTER the opening `"`. Returns index after closing `"`.
fn skip_json_str(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Skips a complete JSON value starting at `i`. Returns index after the value.
fn skip_json_value(bytes: &[u8], mut i: usize) -> Option<usize> {
    i = skip_ws(bytes, i)?;
    match bytes[i] {
        b'"' => skip_json_str(bytes, i + 1),
        b'{' | b'[' => {
            let open = bytes[i];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 1usize;
            i += 1;
            let mut in_str = false;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'\\' if in_str => i += 1,
                    b'"' => in_str = !in_str,
                    b if !in_str && b == open => depth += 1,
                    b if !in_str && b == close => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth == 0 { Some(i) } else { None }
        }
        _ => {
            // number, boolean, null — scan until delimiter
            while i < bytes.len()
                && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n')
            {
                i += 1;
            }
            Some(i)
        }
    }
}

/// Locates the main JSON array in the file.
/// Returns `(pre, array_start, array_end, post)` as byte-index ranges into `bytes`.
/// Supports:
///   - Root is `[...]`
///   - Root is `{"key": [...], ...}` (first array value in the object)
fn find_main_array(bytes: &[u8]) -> Option<(&[u8], usize, usize, &[u8])> {
    let first = skip_ws(bytes, 0)?;
    if bytes[first] == b'[' {
        // Root IS the array; find closing `]`
        let end = skip_json_value(bytes, first)?;
        return Some((&bytes[..first], first, end, &bytes[end..]));
    }
    if bytes[first] != b'{' {
        return None;
    }
    // Root is an object — find the first array-valued key
    let mut i = first + 1;
    loop {
        i = skip_ws(bytes, i)?;
        if bytes[i] == b'}' {
            return None;
        }
        if bytes[i] != b'"' {
            return None;
        }
        i = skip_json_str(bytes, i + 1)?;
        i = skip_ws(bytes, i)?;
        if i >= bytes.len() || bytes[i] != b':' {
            return None;
        }
        i += 1;
        let val_start = skip_ws(bytes, i)?;
        if bytes[val_start] == b'[' {
            let val_end = skip_json_value(bytes, val_start)?;
            return Some((&bytes[..val_start], val_start, val_end, &bytes[val_end..]));
        }
        // Skip this value
        i = skip_json_value(bytes, val_start)?;
        i = skip_ws(bytes, i)?;
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b',' {
            i += 1;
        } else {
            return None;
        }
    }
}

/// Returns `(start, end)` byte ranges of each top-level element within a `[...]` slice.
/// `array_bytes[0]` must be `[`.
fn scan_array_elements(array_bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut i = 1usize; // skip opening '['
    loop {
        // skip whitespace and check for end
        let Some(j) = skip_ws(array_bytes, i) else { break };
        if array_bytes[j] == b']' {
            break;
        }
        // skip a complete element
        let end = match skip_json_value(array_bytes, j) {
            Some(e) => e,
            None => break,
        };
        ranges.push(j..end);
        i = end;
        // skip optional comma
        let Some(k) = skip_ws(array_bytes, i) else { break };
        if array_bytes[k] == b',' {
            i = k + 1;
        } else {
            break;
        }
    }
    ranges
}

impl JsonIndex {
    #[inline]
    fn children_len_for_node(&self, node: &Node) -> u32 {
        if node.kind().is_container() {
            self.container_children[node.value_data as usize] as u32
        } else {
            0
        }
    }

    #[inline]
    fn subtree_len_for_node(&self, node: &Node) -> u32 {
        if node.kind().is_container() {
            self.container_subtrees[node.value_data as usize]
        } else {
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
                + self.container_children.capacity() * std::mem::size_of::<u16>(),
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
        let overflow_idx = self
            .parent_overflow_ids
            .binary_search(&id)
            .expect("parent overflow entry missing");
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

    /// Parallel array element parsing: splits the main array across rayon threads,
    /// parses each chunk independently, then merges by offsetting node IDs.
    /// Falls back silently to sequential parse if the structure is not supported.
    fn try_parallel_from_bytes(
        bytes: &[u8],
        node_cap: usize,
        str_bytes: usize,
        str_max: usize,
    ) -> Option<Result<JsonIndex, String>> {
        const MIN_ELEMENTS: usize = 200;

        // Locate the main JSON array: root is `[...]` or `{"key":[...]}`
        let (pre, array_start, array_end, post) = find_main_array(bytes)?;

        let array_bytes = &bytes[array_start..array_end]; // includes '[' and ']'

        // Collect element byte ranges inside the array
        let elem_ranges = scan_array_elements(array_bytes);
        if elem_ranges.len() < MIN_ELEMENTS {
            return None;
        }

        // Number of threads capped at element count
        let n_threads = rayon::current_num_threads().min(elem_ranges.len());
        let chunk_size = (elem_ranges.len() + n_threads - 1) / n_threads;

        // Each thread gets a JSON array `[elem_i, ..., elem_j]` to parse.
        // element bytes per chunk = sum of element byte lengths + separating commas + 2 brackets
        let per_chunk_nodes = (node_cap / n_threads).max(1024);
        let per_chunk_str = (str_bytes / n_threads).max(4096);

        let sub_results: Vec<Result<JsonIndex, String>> = elem_ranges
            .chunks(chunk_size)
            .map(|chunk_ranges| {
                // Build a synthetic `[elem1,elem2,...]` buffer for this chunk
                let total_bytes: usize = chunk_ranges
                    .iter()
                    .map(|r| r.end - r.start)
                    .sum::<usize>()
                    + chunk_ranges.len() // commas
                    + 2; // `[` and `]`
                let mut buf = Vec::with_capacity(total_bytes);
                buf.push(b'[');
                for (i, r) in chunk_ranges.iter().enumerate() {
                    if i > 0 {
                        buf.push(b',');
                    }
                    buf.extend_from_slice(&array_bytes[r.start..r.end]);
                }
                buf.push(b']');
                let mut de = sonic_rs::Deserializer::from_slice(&buf);
                parse_streaming_with_cap(&mut de, per_chunk_nodes, per_chunk_str, str_max)
                    .map_err(|e| e.to_string())
            })
            .collect();

        // Merge sub-indices -------------------------------------------------------
        // Outer skeleton: parse the file with an empty array in place of the main one.
        // e.g.  {"configurazione":[]}   or   []
        let mut skeleton_buf = Vec::with_capacity(pre.len() + 2 + post.len());
        skeleton_buf.extend_from_slice(pre);
        skeleton_buf.extend_from_slice(b"[]");
        skeleton_buf.extend_from_slice(post);
        let mut skeleton = {
            let mut de = sonic_rs::Deserializer::from_slice(&skeleton_buf);
            match parse_streaming_with_cap(&mut de, 16, 4096, 0) {
                Ok(idx) => idx,
                Err(e) => return Some(Err(e.to_string())),
            }
        };

        // Collect sub-indices (bail out on any error)
        let mut sub_indices = Vec::with_capacity(sub_results.len());
        for r in sub_results {
            match r {
                Ok(idx) => sub_indices.push(idx),
                Err(e) => return Some(Err(e)),
            }
        }

        // Find the array node in the skeleton (it's the last node with kind=Array)
        let array_node_id = skeleton
            .nodes
            .iter()
            .rposition(|n| n.kind() == NodeKind::Array)
            .unwrap_or(0) as u32;

        // Accumulate merged vecs from the skeleton then all sub-indices
        let total_nodes: usize = skeleton.nodes.len()
            + sub_indices.iter().map(|s| s.nodes.len()).sum::<usize>();
        let mut nodes = Vec::with_capacity(total_nodes);
        let mut parent_deltas = Vec::with_capacity(total_nodes);
        let mut parent_overflow_ids: Vec<u32> = Vec::new();
        let mut parent_overflow_values: Vec<u32> = Vec::new();

        // Copy skeleton nodes
        nodes.extend_from_slice(&skeleton.nodes);
        parent_deltas.extend_from_slice(&skeleton.parent_deltas);
        parent_overflow_ids.extend_from_slice(&skeleton.parent_overflow_ids);
        parent_overflow_values.extend_from_slice(&skeleton.parent_overflow_values);

        let mut base_offset = skeleton.nodes.len() as u32;
        let mut total_element_count: u32 = 0;
        let mut array_subtree_len: u32 = 0;

        for sub in &sub_indices {
            // The sub-index root (id=0) is a synthetic Array wrapper; its children
            // are the actual elements. We skip the synthetic root and append its
            // children (ids 1..N) with shifted IDs.
            //
            // The top-level elements (direct children of the synthetic root, delta=1)
            // become direct children of array_node_id in the merged index.
            let sub_node_count = sub.nodes.len() as u32;
            let sub_children = sub.children_len(0);
            total_element_count += sub_children;
            array_subtree_len += sub_node_count - 1; // exclude synthetic root

            for local_id in 1..sub_node_count {
                let node = sub.nodes[local_id as usize].clone();
                let merged_id = base_offset + local_id - 1; // -1 because we skip local root
                nodes.push(node);

                let orig_delta = sub.parent_deltas[local_id as usize];

                // If this node's parent was the synthetic array root (delta points to id 0):
                // remap its parent to array_node_id
                if orig_delta != 0 {
                    let local_parent = local_id - orig_delta as u32;
                    if local_parent == 0 {
                        // Parent is the synthetic root → remap to array_node_id
                        let dist = merged_id - array_node_id;
                        if dist < u8::MAX as u32 {
                            parent_deltas.push(dist as u8);
                        } else {
                            parent_deltas.push(u8::MAX);
                            parent_overflow_ids.push(merged_id);
                            parent_overflow_values.push(array_node_id);
                        }
                    } else {
                        // Relative delta unchanged (shift cancels in subtraction)
                        parent_deltas.push(orig_delta);
                    }
                } else {
                    // This is the synthetic root itself — should have been skipped
                    parent_deltas.push(0);
                }
            }

            // Remap overflow entries that don't reference local root
            for (i, &oid) in sub.parent_overflow_ids.iter().enumerate() {
                if oid == 0 {
                    continue; // synthetic root, already handled above
                }
                let merged_child = base_offset + oid - 1;
                let local_parent = sub.parent_overflow_values[i];
                let merged_parent = if local_parent == 0 {
                    array_node_id
                } else {
                    base_offset + local_parent - 1
                };
                parent_overflow_ids.push(merged_child);
                parent_overflow_values.push(merged_parent);
            }

            base_offset += sub_node_count - 1;
        }

        // Sort overflow tables (binary_search requires sorted order)
        let mut overflow_pairs: Vec<(u32, u32)> = parent_overflow_ids
            .into_iter()
            .zip(parent_overflow_values)
            .collect();
        overflow_pairs.sort_unstable_by_key(|&(id, _)| id);
        let (sorted_overflow_ids, sorted_overflow_values): (Vec<u32>, Vec<u32>) =
            overflow_pairs.into_iter().unzip();

        // Patch the skeleton array node's container metadata
        if let Some(meta_id) = (array_node_id < skeleton.nodes.len() as u32)
            .then(|| skeleton.nodes[array_node_id as usize].value_data as usize)
        {
            if meta_id < skeleton.container_subtrees.len() {
                skeleton.container_subtrees[meta_id] = array_subtree_len;
            }
            if meta_id < skeleton.container_children.len() {
                skeleton.container_children[meta_id] =
                    total_element_count.min(u16::MAX as u32) as u16;
            }
        }

        // Merge container meta from sub-indices
        let mut container_subtrees = skeleton.container_subtrees;
        let mut container_children = skeleton.container_children;
        for sub in &sub_indices {
            // skip the first entry (synthetic root array meta)
            let skip = 1;
            container_subtrees.extend_from_slice(
                sub.container_subtrees.get(skip..).unwrap_or(&[]),
            );
            container_children.extend_from_slice(
                sub.container_children.get(skip..).unwrap_or(&[]),
            );
        }

        // Merge string pools: re-intern all sub-index strings into skeleton pools
        // so IDs are consistent across the merged index.
        // Keys pool: keys from sub-indices need to be remapped into skeleton.keys
        // val_strings pool: same for values.
        //
        // This merge is the most complex part. For correctness we remap each
        // sub-index's string IDs via a translation table built during the merge.
        // We do it sequentially here because InternedStrings is not thread-safe.
        let mut merged_keys = skeleton.keys;
        let mut merged_vals = skeleton.val_strings;
        let mut merged_nums = skeleton.nums_pool;

        // We need to update node ktype (key string id) and value_data (val string id)
        // in the merged nodes. Build per-sub-index remap tables.
        let skeleton_node_count = skeleton.nodes.len();
        let mut merged_node_idx = skeleton_node_count;

        for sub in &sub_indices {
            // Build key ID remap: sub_key_id → merged_key_id
            let key_remap: Vec<u32> = (0..sub.keys.len())
                .map(|id| merged_keys.intern(sub.keys.get(id as u32)))
                .collect();

            // Build val string ID remap
            let val_remap: Vec<u32> = (0..sub.val_strings.len())
                .map(|id| merged_vals.intern(sub.val_strings.get(id as u32)))
                .collect();

            // Build nums remap
            let nums_base = merged_nums.len() as u32;

            // Remap nodes (skipping local id 0 = synthetic root)
            for local_id in 1..sub.nodes.len() {
                let node = &mut nodes[merged_node_idx];
                let kind = node.kind();

                // Remap key
                if let Some(NodeKey::String(kid)) = node.key() {
                    let new_kid = key_remap[kid as usize];
                    // Preserve the kind bits and array-index flag, replace key id
                    node.ktype = (node.ktype & !0x0FFF_FFFF) | (new_kid & 0x0FFF_FFFF);
                }

                // Remap value_data
                match kind {
                    NodeKind::Str => {
                        node.value_data = val_remap[node.value_data as usize];
                    }
                    NodeKind::Num => {
                        // Inline nums: no remap needed
                        // Pool nums: remap index
                        if (node.value_data & INLINE_NUM_FLAG) == 0 {
                            node.value_data += nums_base;
                        }
                    }
                    _ => {}
                }

                merged_node_idx += 1;
            }

            // Append nums pool
            merged_nums.extend_from_slice(&sub.nums_pool);
        }

        // Release val_strings hash table (no longer needed post-parse)
        merged_vals.release_lookup_index();

        Some(Ok(JsonIndex {
            nodes,
            parent_deltas,
            parent_overflow_ids: sorted_overflow_ids,
            parent_overflow_values: sorted_overflow_values,
            container_subtrees,
            container_children,
            keys: merged_keys,
            val_strings: merged_vals,
            nums_pool: merged_nums,
            root: 0,
        }))
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
        #[cfg(not(windows))]
        let file = File::open(path).map_err(|e| e.to_string())?;

        // On Windows use FILE_FLAG_SEQUENTIAL_SCAN (0x0800_0000) so the OS
        // prefetches pages ahead of the sequential SIMD scan — equivalent to
        // MADV_SEQUENTIAL on Unix.
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
                .open(path)
                .map_err(|e| e.to_string())?
        };

        let file_size = file.metadata().map_err(|e| e.to_string())?.len();

        if file_size == 0 {
            return Err("file is empty".to_string());
        }

        let (node_cap, str_bytes) = capacity_hints(file_size);

        // Map the file read-only for SIMD-accelerated parsing.
        // SAFETY: we only read; the file must not be modified while mapped.
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(|e| e.to_string())? };
        #[cfg(unix)]
        let _ = mmap.advise(memmap2::Advice::Sequential);

        let str_max = if file_size >= VERY_LARGE_FILE_THRESHOLD_BYTES {
            VERY_LARGE_FILE_STR_MAX_BYTES
        } else {
            0
        };

        // Try parallel array parse first (gives ~N_threads× speedup on arrays).
        // Falls back to sequential if the file structure is not supported.
        if let Some(result) =
            Self::try_parallel_from_bytes(&mmap[..], node_cap, str_bytes, str_max)
        {
            drop(mmap);
            return result;
        }

        let mut de = sonic_rs::Deserializer::from_slice(&mmap[..]);
        let result = parse_streaming_with_cap(&mut de, node_cap, str_bytes, str_max)
            .map_err(|e| e.to_string());

        // Drop the mmap immediately: releases the 1 GB of virtual address space
        // and the pages brought into RAM by the sequential scan.  All string
        // values have been interned into the compact val_strings pool.
        drop(mmap);
        result
    }

    /// Truly streaming parsing: reads from disk in chunks without buffering in RAM.
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, String> {
        let mut de = serde_json::Deserializer::from_reader(reader);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    pub fn expanded_visible_count(&self) -> usize {
        // root.subtree_len = total descendants. We show all except root itself.
        self.subtree_len(self.root) as usize
    }

    pub fn get_expanded_slice(&self, offset: usize, limit: usize) -> Vec<VisibleSliceRow> {
        if limit == 0 {
            return Vec::new();
        }

        struct Frame {
            next_child_id: u32,
            remaining: u32,
            depth: usize,
        }

        let mut stack: Vec<Frame> = Vec::new();
        let root_children_len = self.children_len(self.root);
        if root_children_len > 0 {
            stack.push(Frame {
                next_child_id: self.root + 1,
                remaining: root_children_len,
                depth: 0,
            });
        }

        let mut skipped = 0usize;
        let mut rows = Vec::with_capacity(limit.min(1024));

        'outer: while let Some(frame) = stack.last_mut() {
            if frame.remaining == 0 {
                stack.pop();
                continue;
            }

            let child_id = frame.next_child_id;
            let depth = frame.depth;
            let child_subtree_len = self.subtree_len(child_id);
            let child_children_len = self.children_len(child_id);

            // Advance frame to next sibling
            frame.next_child_id = child_id + 1 + child_subtree_len;
            frame.remaining -= 1;

            if skipped < offset {
                let subtree_size = child_subtree_len as usize + 1; // this node + descendants
                if skipped + subtree_size <= offset {
                    // Skip entire subtree in O(1)
                    skipped += subtree_size;
                    continue;
                }
                // Enter the subtree to find the offset
                skipped += 1;
                if child_children_len > 0 {
                    stack.push(Frame {
                        next_child_id: child_id + 1,
                        remaining: child_children_len,
                        depth: depth + 1,
                    });
                }
                continue;
            }

            rows.push(VisibleSliceRow {
                id: child_id,
                depth,
            });
            if rows.len() >= limit {
                break 'outer;
            }

            if child_children_len > 0 {
                stack.push(Frame {
                    next_child_id: child_id + 1,
                    remaining: child_children_len,
                    depth: depth + 1,
                });
            }
        }

        rows
    }

    /// Costruisce la rappresentazione JSON raw di un nodo, iterativamente (no ricorsione).
    pub fn build_raw(&self, start_id: u32) -> String {
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
            NodeKind::Object | NodeKind::Array => false,
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
            NodeKind::Object | NodeKind::Array => false,
        }
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
                                NodeKind::Str => finder
                                    .find(self.str_val_of_node(node).as_bytes())
                                    .is_some(),
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
                                NodeKind::Object | NodeKind::Array => false,
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
                    && matches!(node.kind(), NodeKind::Object | NodeKind::Array)
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

    fn compile_object_search_filters(
        &self,
        filters: &[ObjectSearchFilter],
        key_case_sensitive: bool,
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
                if key_case_sensitive && exact_id.is_none() && array_index.is_none() {
                    return None;
                }
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

    fn resolve_relative_path(
        &self,
        start_node_id: u32,
        path_segments: &[CompiledPathSegment],
        key_case_sensitive: bool,
    ) -> Option<u32> {
        let mut current = start_node_id;
        for segment in path_segments {
            let parent = &self.nodes[current as usize];
            current = match parent.kind() {
                NodeKind::Array => {
                    let wanted_index = segment.array_index?;
                    self.children_iter(current).find(|&child_id| {
                        self.nodes[child_id as usize].array_index() == Some(wanted_index)
                    })?
                }
                NodeKind::Object => self.children_iter(current).find(|&child_id| {
                    let Some(child_key_id) = self.nodes[child_id as usize].string_key_id() else {
                        return false;
                    };
                    if key_case_sensitive {
                        return Some(child_key_id) == segment.exact_id;
                    }
                    let child_key = self.keys.get(child_key_id);
                    matches_text(child_key, &segment.raw, &segment.lower, false, true)
                })?,
                _ => return None,
            };
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
        let node = &self.nodes[target_id as usize];

        match filter.operator {
            ObjectSearchOperator::Exists => true,
            ObjectSearchOperator::Regex => filter
                .regex
                .as_ref()
                .is_some_and(|regex| self.value_matches_regex(node, regex)),
            ObjectSearchOperator::Contains | ObjectSearchOperator::Equals => {
                let needle = filter.value_cmp.as_deref().unwrap_or_default();
                self.value_matches_query(
                    node,
                    needle,
                    needle,
                    value_case_sensitive,
                    filter.operator == ObjectSearchOperator::Equals,
                    filter.value_num,
                )
            }
        }
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
        })
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

        let mut suggestions: Vec<&str> = (0..self.keys.len() as u32)
            .map(|id| self.keys.get(id))
            .filter(|candidate| {
                starts_with_case_insensitive(candidate, segment_prefix, &prefix_lower)
            })
            .collect();

        suggestions.sort_unstable_by(|a, b| {
            let a_is_numeric = a.chars().all(|ch| ch.is_ascii_digit());
            let b_is_numeric = b.chars().all(|ch| ch.is_ascii_digit());
            let a_rank = (
                !a.starts_with(segment_prefix),
                segment_prefix.is_empty() && a_is_numeric,
                a.len(),
                *a,
            );
            let b_rank = (
                !b.starts_with(segment_prefix),
                segment_prefix.is_empty() && b_is_numeric,
                b.len(),
                *b,
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
}

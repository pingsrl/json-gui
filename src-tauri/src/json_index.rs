use rayon::prelude::*;
use regex::Regex;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::File;
use std::rc::Rc;

// ---- InternedStrings ----
// Compact pool for string values: a single Vec<u8> for bytes + open-addressing
// hash table on Vec<u32> (4 bytes/slot). Zero allocations per string,
// zero byte doubling as in HashMap<String> or HashMap<Arc<str>>.
// Memory for N unique strings, T total bytes: T + 13N bytes.

pub struct InternedStrings {
    pub data: Vec<u8>,     // bytes of all unique strings, concatenated
    pub offsets: Vec<u32>, // start of each string in data
    pub lens: Vec<u32>,    // byte length of each string
    index: Vec<u32>,       // open-addressing hash table: slot → (id+1), 0=empty
    index_mask: u32,       // capacity - 1 (capacity is a power of 2)
}

impl InternedStrings {
    pub fn new() -> Self {
        Self::with_capacity(0, 0)
    }

    /// Pre-allocates internal buffers to avoid doublings during parsing.
    /// `n_strings`: estimated number of unique strings (for offsets/lens/index).
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
            offsets: Vec::with_capacity(n_strings),
            lens: Vec::with_capacity(n_strings),
            index: vec![0u32; hash_cap],
            index_mask: (hash_cap - 1) as u32,
        }
    }

    #[inline]
    fn fnv1a(s: &[u8]) -> u32 {
        let mut h = 2166136261u32;
        for &b in s {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        h
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        // Grow the index if load > 75%
        if (self.offsets.len() + 1) * 4 > self.index.len() * 3 {
            self.grow_index();
        }
        let bytes = s.as_bytes();
        let hash = Self::fnv1a(bytes);
        let mut slot = hash & self.index_mask;
        loop {
            let entry = self.index[slot as usize];
            if entry == 0 {
                // Empty slot: insert new string
                let id = self.offsets.len() as u32;
                self.offsets.push(self.data.len() as u32);
                self.lens.push(bytes.len() as u32);
                self.data.extend_from_slice(bytes);
                self.index[slot as usize] = id + 1;
                return id;
            }
            // Check if the already-stored string matches
            let eid = (entry - 1) as usize;
            let start = self.offsets[eid] as usize;
            let len = self.lens[eid] as usize;
            if &self.data[start..start + len] == bytes {
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
        for id in 0..self.offsets.len() {
            let start = self.offsets[id] as usize;
            let len = self.lens[id] as usize;
            let hash = Self::fnv1a(&self.data[start..start + len]);
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
        let len = self.lens[id as usize] as usize;
        // SAFETY: only valid UTF-8 strings inserted via intern(&str)
        unsafe { std::str::from_utf8_unchecked(&self.data[start..start + len]) }
    }

    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    pub fn heap_bytes_estimate(&self) -> usize {
        self.data.capacity()
            + self.offsets.capacity() * std::mem::size_of::<u32>()
            + self.lens.capacity() * std::mem::size_of::<u32>()
            + self.index.capacity() * std::mem::size_of::<u32>()
    }

    /// Looks up the id of an already-interned string without inserting it. O(1) amortized.
    pub fn id_of(&self, s: &str) -> Option<u32> {
        if self.offsets.is_empty() {
            return None;
        }
        let bytes = s.as_bytes();
        let hash = Self::fnv1a(bytes);
        let mut slot = hash & self.index_mask;
        loop {
            let entry = self.index[slot as usize];
            if entry == 0 {
                return None; // not found
            }
            let eid = (entry - 1) as usize;
            let start = self.offsets[eid] as usize;
            let len = self.lens[eid] as usize;
            if &self.data[start..start + len] == bytes {
                return Some(eid as u32);
            }
            slot = (slot + 1) & self.index_mask;
        }
    }
}

// ---- Node (16 bytes, 4×u32, no padding) ----
//
// ktype  bits[31:29] = NodeKind (0..5)
// ktype  bits[28:0]  = key data:
//                      - string key id       if bit 28 = 0
//                      - array index         if bit 28 = 1
//                      - NO_KEY sentinel     if all 29 bits = 1
// value_data: Str→val_strings id, Num→nums_pool idx, Bool→0/1, others→0
//
// Note: the node id (index in the Vec) always coincides with the preorder DFS index,
// because the streaming parser allocates the parent before its children and children in order.

pub const NO_KEY: u32 = 0x1FFF_FFFF; // sentinel: no key (29 bits all ones)
const ARRAY_INDEX_FLAG: u32 = 0x1000_0000;
const KEY_DATA_MASK: u32 = 0x0FFF_FFFF;

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
    Num = 3,  // value_data = index into nums_pool
    Bool = 4, // value_data = 0 (false) or 1 (true)
    Null = 5,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub ktype: u32,      // bits[31:29]=NodeKind, bits[28:0]=packed NodeKey
    pub value_data: u32, // Str→str_id, Num→num_idx, Bool→0/1
    pub parent: u32,     // u32::MAX for root
    pub children_len: u32,
    pub subtree_len: u32,
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
}

/// Zero-alloc iterator over the direct children of a node.
/// Uses the DFS preorder layout: first child = id+1,
/// next sibling = cur + 1 + nodes[cur].subtree_len.
pub struct ChildrenIter<'a> {
    nodes: &'a [Node],
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
        self.cur += self.nodes[id as usize].subtree_len + 1;
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
    keys: InternedStrings,
    val_strings: InternedStrings,
    nums_pool: Vec<f64>, // f64 pool: NodeKind::Num → nums_pool[value_data]
}

impl StreamCtx {
    /// Pre-allocates the main Vecs to avoid doublings during parsing.
    /// `node_cap`    = estimated nodes (file_size / 50).
    /// `str_bytes`   = estimated unique bytes for val_strings (file_size / 10).
    fn with_capacity(node_cap: usize, str_bytes: usize) -> Self {
        // JSON keys: few short strings (e.g. field names). Estimate: 1% of nodes, 20 bytes/key.
        let key_n = (node_cap / 100).max(64);
        let key_bytes = key_n * 20;
        // String values: ~30% of nodes, deduplicated bytes already passed as str_bytes.
        let val_n = node_cap * 3 / 10;
        // Numbers: ~20% of nodes.
        let num_cap = node_cap / 5;
        Self {
            nodes: Vec::with_capacity(node_cap),
            keys: InternedStrings::with_capacity(key_n, key_bytes),
            val_strings: InternedStrings::with_capacity(val_n, str_bytes),
            nums_pool: Vec::with_capacity(num_cap),
        }
    }

    fn alloc(&mut self, kind: NodeKind, key: Option<NodeKey>, value_data: u32, parent: u32) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            ktype: Node::make_ktype(kind, key),
            value_data,
            parent,
            children_len: 0,
            subtree_len: 0,
        });
        if parent != u32::MAX {
            self.nodes[parent as usize].children_len += 1;
        }
        id
    }
}

struct ValSeed {
    ctx: Rc<RefCell<StreamCtx>>,
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
    ctx: Rc<RefCell<StreamCtx>>,
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
            .borrow_mut()
            .alloc(NodeKind::Bool, self.key, v as u32, self.parent))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v as f64);
        Ok(ctx.alloc(NodeKind::Num, self.key, nid, self.parent))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v as f64);
        Ok(ctx.alloc(NodeKind::Num, self.key, nid, self.parent))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v);
        Ok(ctx.alloc(NodeKind::Num, self.key, nid, self.parent))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
        let sid = self.ctx.borrow_mut().val_strings.intern(v);
        Ok(self
            .ctx
            .borrow_mut()
            .alloc(NodeKind::Str, self.key, sid, self.parent))
    }
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<u32, E> {
        self.visit_str(v)
    }
    fn visit_unit<E: de::Error>(self) -> Result<u32, E> {
        Ok(self
            .ctx
            .borrow_mut()
            .alloc(NodeKind::Null, self.key, 0, self.parent))
    }
    fn visit_none<E: de::Error>(self) -> Result<u32, E> {
        Ok(self
            .ctx
            .borrow_mut()
            .alloc(NodeKind::Null, self.key, 0, self.parent))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<u32, A::Error> {
        let id = {
            let mut ctx = self.ctx.borrow_mut();
            ctx.alloc(NodeKind::Object, self.key, 0, self.parent)
        };
        while let Some(key_str) = map.next_key::<Cow<'de, str>>()? {
            let kid = self.ctx.borrow_mut().keys.intern(&key_str);
            map.next_value_seed(ValSeed {
                ctx: Rc::clone(&self.ctx),
                parent: id,
                key: Some(NodeKey::String(kid)),
            })?;
        }
        // Set subtree_len AFTER all children are allocated
        {
            let mut ctx = self.ctx.borrow_mut();
            ctx.nodes[id as usize].subtree_len = ctx.nodes.len() as u32 - id - 1;
        }
        Ok(id)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u32, A::Error> {
        let id = {
            let mut ctx = self.ctx.borrow_mut();
            ctx.alloc(NodeKind::Array, self.key, 0, self.parent)
        };
        let mut index = 0usize;
        loop {
            if index >= KEY_DATA_MASK as usize {
                return Err(de::Error::custom(
                    "array index exceeds inline storage capacity",
                ));
            }
            if seq
                .next_element_seed(ValSeed {
                    ctx: Rc::clone(&self.ctx),
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
        {
            let mut ctx = self.ctx.borrow_mut();
            ctx.nodes[id as usize].subtree_len = ctx.nodes.len() as u32 - id - 1;
        }
        Ok(id)
    }
}

/// Finalizes the JsonIndex: trivial finish since subtree_len is already set during streaming.
/// For leaf nodes (Bool, Str, Num, Null) subtree_len remains 0 (set in alloc), which is correct.
fn finish_index(ctx: StreamCtx) -> JsonIndex {
    let StreamCtx {
        nodes,
        keys,
        val_strings,
        nums_pool,
    } = ctx;
    JsonIndex {
        nodes,
        keys,
        val_strings,
        nums_pool,
        root: 0,
    }
}

fn parse_streaming<'de, D: de::Deserializer<'de>>(de: D) -> Result<JsonIndex, D::Error> {
    parse_streaming_with_cap(de, 0, 0)
}

fn parse_streaming_with_cap<'de, D: de::Deserializer<'de>>(
    de: D,
    node_cap: usize,
    str_bytes: usize,
) -> Result<JsonIndex, D::Error> {
    let ctx = Rc::new(RefCell::new(StreamCtx::with_capacity(node_cap, str_bytes)));
    de.deserialize_any(ValVisitor {
        ctx: Rc::clone(&ctx),
        parent: u32::MAX,
        key: None,
    })?;
    Ok(finish_index(
        Rc::try_unwrap(ctx)
            .ok()
            .expect("ctx: more than one Rc reference")
            .into_inner(),
    ))
}

// ---- JsonIndex ----

pub struct JsonIndex {
    pub nodes: Vec<Node>,
    pub keys: InternedStrings,
    pub val_strings: InternedStrings, // string values: compact zero-alloc pool
    pub nums_pool: Vec<f64>,          // numeric values: NodeKind::Num(idx) → nums_pool[idx]
    pub root: u32,
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
    /// Returns the ids of direct children of `id`, computed from DFS preorder.
    /// First child = id+1; next sibling = prev_child + prev_child.subtree_len + 1.
    #[inline]
    pub fn get_children_slice(&self, id: u32) -> Vec<u32> {
        let node = &self.nodes[id as usize];
        let count = node.children_len as usize;
        let mut out = Vec::with_capacity(count);
        let mut cur = id + 1;
        for _ in 0..count {
            out.push(cur);
            cur += self.nodes[cur as usize].subtree_len + 1;
        }
        out
    }

    /// Zero-alloc iterator over direct children of `id`.
    #[inline]
    pub fn children_iter(&self, id: u32) -> ChildrenIter<'_> {
        let node = &self.nodes[id as usize];
        ChildrenIter {
            nodes: &self.nodes,
            cur: id + 1,
            remaining: node.children_len,
        }
    }

    pub fn heap_bytes_estimate(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.keys.heap_bytes_estimate()
            + self.val_strings.heap_bytes_estimate()
            + self.nums_pool.capacity() * std::mem::size_of::<f64>()
    }

    /// Returns the direct parent of `id`. O(1) field lookup.
    pub fn parent_of(&self, id: u32) -> Option<u32> {
        let p = self.nodes[id as usize].parent;
        if p == u32::MAX { None } else { Some(p) }
    }

    fn scoped_nodes(&self, scope_node_id: Option<u32>) -> (u32, &[Node]) {
        match scope_node_id {
            Some(scope_id) => {
                let start = scope_id as usize;
                let len = self.nodes[start].subtree_len as usize + 1;
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
        let file = File::open(path).map_err(|e| e.to_string())?;
        let file_size = file.metadata().map_err(|e| e.to_string())?.len();

        if file_size == 0 {
            return Err("file is empty".to_string());
        }

        // Conservative capacity hints so the internal Vecs grow at most once.
        let node_cap = (file_size / 50).min(200_000_000) as usize;
        let str_bytes = (file_size / 10).min(500_000_000) as usize;

        // Map the file read-only. Safety: we only read, and the file is not
        // modified during this call.
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(|e| e.to_string())? };

        let mut de = sonic_rs::Deserializer::from_slice(&mmap[..]);
        let result =
            parse_streaming_with_cap(&mut de, node_cap, str_bytes).map_err(|e| e.to_string());

        // mmap is dropped here: virtual address space released, no persistent overhead.
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
        self.nodes[self.root as usize].subtree_len as usize
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

        let root_node = &self.nodes[self.root as usize];
        let mut stack: Vec<Frame> = Vec::new();
        if root_node.children_len > 0 {
            stack.push(Frame {
                next_child_id: self.root + 1,
                remaining: root_node.children_len,
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
            let child = &self.nodes[child_id as usize];
            let depth = frame.depth;

            // Advance frame to next sibling
            frame.next_child_id = child_id + 1 + child.subtree_len;
            frame.remaining -= 1;

            if skipped < offset {
                let subtree_size = child.subtree_len as usize + 1; // this node + descendants
                if skipped + subtree_size <= offset {
                    // Skip entire subtree in O(1)
                    skipped += subtree_size;
                    continue;
                }
                // Enter the subtree to find the offset
                skipped += 1;
                if child.children_len > 0 {
                    stack.push(Frame {
                        next_child_id: child_id + 1,
                        remaining: child.children_len,
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

            if child.children_len > 0 {
                stack.push(Frame {
                    next_child_id: child_id + 1,
                    remaining: child.children_len,
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
                    let count = node.children_len;
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
                        let first_subtree = self.nodes[first as usize].subtree_len;
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
                    json_escape_into(&mut out, self.val_strings.get(node.value_data));
                    out.push('"');
                }
                NodeKind::Num => {
                    let f = self.nums_pool[node.value_data as usize];
                    if f.fract() == 0.0 && f.abs() < 1e15 {
                        let _ = write!(out, "{}", f as i64);
                    } else {
                        let _ = write!(out, "{f}");
                    }
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
                            frame.next_child_id = next + 1 + self.nodes[next as usize].subtree_len;
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
                let _ = write!(out, "{index}");
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
            NodeKind::Str => re.is_match(self.val_strings.get(node.value_data)),
            NodeKind::Num => {
                let mut text = String::new();
                let _ = write!(text, "{}", self.nums_pool[node.value_data as usize]);
                re.is_match(&text)
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
                self.val_strings.get(node.value_data),
                query,
                query_lower,
                case_sensitive,
                exact_match,
            ),
            NodeKind::Num => {
                let value = self.nums_pool[node.value_data as usize];
                if exact_match {
                    return exact_number.is_some_and(|expected| value == expected);
                }
                let mut text = String::new();
                let _ = write!(text, "{value}");
                matches_text(&text, query, query_lower, case_sensitive, false)
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
        suggestions.dedup();

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
}

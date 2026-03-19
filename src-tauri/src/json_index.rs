use rayon::prelude::*;
use std::io::BufReader;
use regex::Regex;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::rc::Rc;
use std::sync::Arc;

// ---- StringPool ----
// Uses Arc<str> to avoid byte duplication between Vec and HashMap:
// Vec and HashMap share the same heap via reference counting.

pub struct StringPool {
    pub strings: Vec<Arc<str>>,
    map: HashMap<Arc<str>, u32>,
}

impl StringPool {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            map: HashMap::new(),
        }
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = self.strings.len() as u32;
        let arc: Arc<str> = s.into();
        self.strings.push(Arc::clone(&arc));
        self.map.insert(arc, id);
        id
    }

    pub fn get(&self, id: u32) -> &str {
        &self.strings[id as usize]
    }

    pub fn id_of(&self, s: &str) -> Option<u32> {
        self.map.get(s).copied()
    }
}

// ---- InternedStrings ----
// Compact pool for string values: a single Vec<u8> for bytes + open-addressing
// hash table on Vec<u32> (4 bytes/slot). Zero allocations per string,
// zero byte doubling as in HashMap<String> or HashMap<Arc<str>>.
// Memory for N unique strings, T total bytes: T + 13N bytes.

pub struct InternedStrings {
    pub data: Vec<u8>,    // bytes of all unique strings, concatenated
    pub offsets: Vec<u32>, // start of each string in data
    pub lens: Vec<u32>,   // byte length of each string
    index: Vec<u32>,      // open-addressing hash table: slot → (id+1), 0=empty
    index_mask: u32,      // capacity - 1 (capacity is a power of 2)
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

// ---- NodeValue ----

// NodeValue uses u32 for all payloads (Str=index in val_strings, Num=index in nums_pool)
// so the max payload is 4 bytes → enum occupies 8 bytes instead of 16 (with direct f64).
// Savings: 21M nodes × 8 bytes = ~168 MB on a 1 GB file.
#[derive(Debug, Clone)]
pub enum NodeValue {
    Object,
    Array,
    Str(u32),  // index into JsonIndex.val_strings
    Num(u32),  // index into JsonIndex.nums_pool: Vec<f64>
    Bool(bool),
    Null,
}

// ---- Node ----
//
// Note: the node id (index in the Vec) always coincides with the preorder DFS index,
// because the streaming parser allocates the parent before its children and children in order.
// It is therefore not necessary to store preorder_index separately.

#[derive(Debug, Clone)]
pub struct Node {
    // id removed - use the index in the Vec (= preorder DFS index)
    pub parent: u32,          // u32::MAX = root node (no parent)
    pub key: u32,             // u32::MAX = no key
    pub value: NodeValue,
    pub children_start: u32,
    pub children_len: u32,
    pub subtree_len: u32,
}

// ---- Streaming parser: zero intermediate allocations ----
//
// Final Nodes are allocated DIRECTLY during DFS streaming.
// No temporary linked-lists needed: children_len values are updated in alloc(),
// finish_index builds children_arena with prefix-sum + fill-by-id-order
// using a single temporary Vec<u32> (pos[]) instead of 3 × 84 MB.

struct StreamCtx {
    nodes: Vec<Node>,
    keys: InternedStrings,
    val_strings: InternedStrings,
    nums_pool: Vec<f64>,     // f64 pool: NodeValue::Num(idx) → nums_pool[idx]
}

impl StreamCtx {
    fn new() -> Self {
        Self::with_capacity(0, 0)
    }

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

    fn alloc(&mut self, value: NodeValue, parent: u32, key: u32) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            parent,
            key,
            value,
            children_start: 0,  // filled in finish_index
            children_len: 0,    // incremented below
            subtree_len: 1,     // filled in finish_index
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
    key: u32,
}

impl<'de> DeserializeSeed<'de> for ValSeed {
    type Value = u32;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<u32, D::Error> {
        de.deserialize_any(ValVisitor { ctx: self.ctx, parent: self.parent, key: self.key })
    }
}

struct ValVisitor {
    ctx: Rc<RefCell<StreamCtx>>,
    parent: u32,
    key: u32,
}

impl<'de> Visitor<'de> for ValVisitor {
    type Value = u32;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a JSON value")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<u32, E> {
        Ok(self.ctx.borrow_mut().alloc(NodeValue::Bool(v), self.parent, self.key))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v as f64);
        Ok(ctx.alloc(NodeValue::Num(nid), self.parent, self.key))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v as f64);
        Ok(ctx.alloc(NodeValue::Num(nid), self.parent, self.key))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<u32, E> {
        let mut ctx = self.ctx.borrow_mut();
        let nid = ctx.nums_pool.len() as u32;
        ctx.nums_pool.push(v);
        Ok(ctx.alloc(NodeValue::Num(nid), self.parent, self.key))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
        let sid = self.ctx.borrow_mut().val_strings.intern(v);
        Ok(self.ctx.borrow_mut().alloc(NodeValue::Str(sid), self.parent, self.key))
    }
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<u32, E> {
        self.visit_str(v)
    }
    fn visit_unit<E: de::Error>(self) -> Result<u32, E> {
        Ok(self.ctx.borrow_mut().alloc(NodeValue::Null, self.parent, self.key))
    }
    fn visit_none<E: de::Error>(self) -> Result<u32, E> {
        Ok(self.ctx.borrow_mut().alloc(NodeValue::Null, self.parent, self.key))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<u32, A::Error> {
        let id = self.ctx.borrow_mut().alloc(NodeValue::Object, self.parent, self.key);
        while let Some(key_str) = map.next_key::<String>()? {
            let kid = self.ctx.borrow_mut().keys.intern(&key_str);
            map.next_value_seed(ValSeed { ctx: Rc::clone(&self.ctx), parent: id, key: kid })?;
        }
        Ok(id)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u32, A::Error> {
        let id = self.ctx.borrow_mut().alloc(NodeValue::Array, self.parent, self.key);
        let mut index = 0usize;
        loop {
            let kid = self.ctx.borrow_mut().keys.intern(&index.to_string());
            if seq
                .next_element_seed(ValSeed { ctx: Rc::clone(&self.ctx), parent: id, key: kid })?
                .is_none()
            {
                break;
            }
            index += 1;
        }
        Ok(id)
    }
}

/// Finalizes the JsonIndex: builds children_arena with prefix-sum + fill-by-id-order,
/// then computes subtree_len bottom-up.
/// Zero temporary Vecs: reuses children_start as a fill cursor, then recomputes it.
/// The property that makes the algorithm correct: the streaming parser allocates nodes in
/// DFS preorder, so children of the same parent have increasing ids in visit order.
fn finish_index(ctx: StreamCtx) -> JsonIndex {
    let StreamCtx { mut nodes, keys, val_strings, nums_pool } = ctx;

    let n = nodes.len();
    let total_children: usize = nodes.iter().map(|nd| nd.children_len as usize).sum();

    // Step 1: children_start = prefix-sum of children_len
    {
        let mut sum = 0u32;
        for node in &mut nodes {
            node.children_start = sum;
            sum += node.children_len;
        }
    }

    // Passo 2: riempie children_arena in ordine crescente di id.
    // Usa children_start come cursore temporaneo (viene sovrascritto e poi ricalcolato).
    // Zero allocazioni extra: nessun Vec<u32> pos[] separato.
    let mut children_arena: Vec<u32> = vec![0u32; total_children];
    for id in 1..n as u32 {
        let parent = nodes[id as usize].parent;
        let slot = nodes[parent as usize].children_start as usize;
        children_arena[slot] = id;
        nodes[parent as usize].children_start += 1;
    }

    // Passo 3: ripristina children_start ai valori corretti (prefix-sum)
    {
        let mut sum = 0u32;
        for node in &mut nodes {
            let len = node.children_len;
            node.children_start = sum;
            sum += len;
        }
    }

    // Calcola subtree_len (bottom-up: foglie prima, poi risale)
    for idx in (0..n).rev() {
        let nd = &nodes[idx];
        let s = nd.children_start as usize;
        let e = s + nd.children_len as usize;
        let sub: u32 = children_arena[s..e].iter().map(|&c| nodes[c as usize].subtree_len).sum();
        nodes[idx].subtree_len = 1 + sub;
    }

    JsonIndex { nodes, children: children_arena, keys, val_strings, nums_pool, root: 0 }
}

fn parse_streaming<'de, D: de::Deserializer<'de>>(de: D) -> Result<JsonIndex, D::Error> {
    parse_streaming_with_cap(de, 0, 0)
}

fn parse_streaming_with_cap<'de, D: de::Deserializer<'de>>(de: D, node_cap: usize, str_bytes: usize) -> Result<JsonIndex, D::Error> {
    let ctx = Rc::new(RefCell::new(StreamCtx::with_capacity(node_cap, str_bytes)));
    de.deserialize_any(ValVisitor { ctx: Rc::clone(&ctx), parent: u32::MAX, key: u32::MAX })?;
    Ok(finish_index(Rc::try_unwrap(ctx).ok().expect("ctx: more than one Rc reference").into_inner()))
}

// ---- JsonIndex ----

pub struct JsonIndex {
    pub nodes: Vec<Node>,
    pub children: Vec<u32>,
    pub keys: InternedStrings,
    pub val_strings: InternedStrings, // stringhe dei valori: pool compatto zero-alloc
    pub nums_pool: Vec<f64>,          // valori numerici: NodeValue::Num(idx) → nums_pool[idx]
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
    regex: Option<Regex>,
}

struct CompiledPathSegment {
    raw: String,
    lower: String,
    exact_id: Option<u32>,
}

impl JsonIndex {
    pub fn get_children_slice(&self, node_id: u32) -> &[u32] {
        let node = &self.nodes[node_id as usize];
        let start = node.children_start as usize;
        let end = start + node.children_len as usize;
        &self.children[start..end]
    }

    pub fn from_str(json: &str) -> Result<Self, String> {
        let mut de = serde_json::Deserializer::from_str(json);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, String> {
        let mut de = serde_json::Deserializer::from_slice(bytes);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    /// Carica da file: BufReader da 1MB per parsing streaming a basso consumo RAM.
    /// Stima il numero di nodi dal file_size (≈ 1 nodo ogni 50 byte) per pre-allocare
    /// i Vec interni ed evitare il raddoppio della capacità durante il parsing.
    pub fn from_file(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let file_size = file.metadata().map_err(|e| e.to_string())?.len();
        // Stima conservativa: 1 nodo ogni 50 byte. Pone la capacità iniziale senza
        // sovra-allocare, così il Vec cresce al massimo una volta invece di fare
        // log2(N) raddoppi da 0 → 32M con un picco di capacità 2× necessario.
        let node_cap = (file_size / 50).min(200_000_000) as usize;
        // Stima byte unici per val_strings: ~10% del file (deduplicazione + struttura JSON)
        let str_bytes = (file_size / 10).min(500_000_000) as usize;
        let reader = BufReader::with_capacity(1 << 20, file);
        let mut de = serde_json::Deserializer::from_reader(reader);
        parse_streaming_with_cap(&mut de, node_cap, str_bytes).map_err(|e| e.to_string())
    }

    /// Parsing veramente streaming: legge dal disco a chunk senza buffering in RAM.
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, String> {
        let mut de = serde_json::Deserializer::from_reader(reader);
        parse_streaming(&mut de).map_err(|e| e.to_string())
    }

    pub fn expanded_visible_count(&self) -> usize {
        self.nodes[self.root as usize].subtree_len.saturating_sub(1) as usize
    }

    pub fn get_expanded_slice(&self, offset: usize, limit: usize) -> Vec<VisibleSliceRow> {
        if limit == 0 {
            return Vec::new();
        }

        struct Frame {
            parent_id: u32,
            next_child_index: usize,
            depth: usize,
        }

        let mut rows = Vec::with_capacity(limit);
        let mut skip = offset as u32;
        let mut stack = vec![Frame {
            parent_id: self.root,
            next_child_index: 0,
            depth: 0,
        }];

        while !stack.is_empty() {
            let (child_id, depth) = {
                let frame = stack.last_mut().unwrap();
                let children = self.get_children_slice(frame.parent_id);
                if frame.next_child_index >= children.len() {
                    stack.pop();
                    continue;
                }

                let child_id = children[frame.next_child_index];
                frame.next_child_index += 1;
                (child_id, frame.depth)
            };
            let child = &self.nodes[child_id as usize];

            if skip >= child.subtree_len {
                skip -= child.subtree_len;
                continue;
            }

            if skip == 0 {
                rows.push(VisibleSliceRow {
                    id: child_id,
                    depth,
                });
                if rows.len() >= limit {
                    break;
                }
            } else {
                skip -= 1;
            }

            if child.children_len > 0 {
                stack.push(Frame {
                    parent_id: child_id,
                    next_child_index: 0,
                    depth: depth + 1,
                });
            }
        }

        rows
    }

    /// Costruisce la rappresentazione JSON raw di un nodo, iterativamente (no ricorsione).
    pub fn build_raw(&self, start_id: u32) -> String {
        enum Task {
            Node(u32),
            Literal(&'static str),
            Key(u32),
        }

        // Stima dimensione output per pre-allocare
        let mut out = String::with_capacity(256);
        let mut stack: Vec<Task> = Vec::with_capacity(64);
        stack.push(Task::Node(start_id));

        while let Some(task) = stack.pop() {
            match task {
                Task::Literal(s) => out.push_str(s),
                Task::Key(kid) => {
                    out.push('"');
                    let k = self.keys.get(kid);
                    json_escape_into(&mut out, k);
                    out.push_str("\":");
                }
                Task::Node(id) => {
                    let node = &self.nodes[id as usize];
                    let children_slice = self.get_children_slice(id);
                    match &node.value {
                        NodeValue::Object => {
                            if children_slice.is_empty() {
                                out.push_str("{}");
                            } else {
                                // Push in reverse order because stack is LIFO
                                stack.push(Task::Literal("}"));
                                for (i, &child_id) in children_slice.iter().enumerate().rev() {
                                    stack.push(Task::Node(child_id));
                                    let child_node = &self.nodes[child_id as usize];
                                    if child_node.key != u32::MAX {
                                        stack.push(Task::Key(child_node.key));
                                    }
                                    if i > 0 {
                                        stack.push(Task::Literal(","));
                                    }
                                }
                                out.push('{');
                            }
                        }
                        NodeValue::Array => {
                            if children_slice.is_empty() {
                                out.push_str("[]");
                            } else {
                                stack.push(Task::Literal("]"));
                                for (i, &child_id) in children_slice.iter().enumerate().rev() {
                                    stack.push(Task::Node(child_id));
                                    if i > 0 {
                                        stack.push(Task::Literal(","));
                                    }
                                }
                                out.push('[');
                            }
                        }
                        NodeValue::Str(sid) => {
                            out.push('"');
                            json_escape_into(&mut out, self.val_strings.get(*sid));
                            out.push('"');
                        }
                        NodeValue::Num(nid) => {
                            let f = self.nums_pool[*nid as usize];
                            if f.fract() == 0.0 && f.abs() < 1e15 {
                                let i = f as i64;
                                out.push_str(&i.to_string());
                            } else {
                                out.push_str(&f.to_string());
                            }
                        }
                        NodeValue::Bool(b) => {
                            out.push_str(if *b { "true" } else { "false" });
                        }
                        NodeValue::Null => {
                            out.push_str("null");
                        }
                    }
                }
            }
        }

        out
    }

    pub fn get_path(&self, node_id: u32) -> String {
        // Accumula key IDs senza clonare stringhe
        let mut key_ids: Vec<u32> = Vec::with_capacity(16);
        let mut current = node_id;
        loop {
            let node = &self.nodes[current as usize];
            if node.key != u32::MAX {
                key_ids.push(node.key);
            }
            if node.parent == u32::MAX { break; }
            current = node.parent;
        }
        key_ids.reverse();
        if key_ids.is_empty() {
            "$".to_string()
        } else {
            // Pre-alloca: "$." + somma lunghezze + separatori
            let total_len: usize = key_ids.iter().map(|&k| self.keys.get(k).len()).sum::<usize>()
                + key_ids.len() // separatori "."
                + 2; // "$."
            let mut out = String::with_capacity(total_len);
            out.push('$');
            for kid in key_ids {
                out.push('.');
                out.push_str(self.keys.get(kid));
            }
            out
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
    ) -> Vec<(u32, String)> {
        let scope_node_id = match path.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => match self.resolve_path(path) {
                Some(node_id) => Some(node_id),
                None => return vec![],
            },
            None => None,
        };

        let results: Vec<(u32, String)> = if use_regex {
            let mut flags = String::new();
            if !case_sensitive { flags.push('i'); }
            if multiline { flags.push('m'); }
            if dot_all { flags.push('s'); }
            let pattern = if flags.is_empty() {
                query.to_string()
            } else {
                format!("(?{}){}", flags, query)
            };
            let re = match Regex::new(&pattern) {
                Ok(r) => r,
                Err(_) => return vec![],
            };
            self.nodes
                .par_iter()
                .enumerate()
                .filter_map(|(idx, node)| {
                    let node_id = idx as u32;
                    if let Some(scope_id) = scope_node_id {
                        if !self.is_descendant_or_self(node_id, scope_id) {
                            return None;
                        }
                    }

                    let key_str: Option<&str> = if node.key != u32::MAX { Some(self.keys.get(node.key)) } else { None };
                    let matches_key = (target == "keys" || target == "both")
                        && key_str.map(|k| re.is_match(k)).unwrap_or(false);

                    let matches_value = (target == "values" || target == "both")
                        && match &node.value {
                            NodeValue::Str(sid) => re.is_match(self.val_strings.get(*sid)),
                            NodeValue::Num(nid) => re.is_match(&self.nums_pool[*nid as usize].to_string()),
                            NodeValue::Bool(b) => re.is_match(&b.to_string()),
                            _ => false,
                        };

                    if matches_key || matches_value {
                        Some((node_id, self.get_path(node_id)))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            let query_lower = if case_sensitive {
                query.to_string()
            } else {
                query.to_lowercase()
            };

            self.nodes
                .par_iter()
                .enumerate()
                .filter_map(|(idx, node)| {
                    let node_id = idx as u32;
                    if let Some(scope_id) = scope_node_id {
                        if !self.is_descendant_or_self(node_id, scope_id) {
                            return None;
                        }
                    }

                    let key_str: Option<&str> = if node.key != u32::MAX { Some(self.keys.get(node.key)) } else { None };
                    let matches_key = if target == "keys" || target == "both" {
                        key_str
                            .map(|k| {
                                let k_cmp = if case_sensitive {
                                    k.to_string()
                                } else {
                                    k.to_lowercase()
                                };
                                if exact_match {
                                    k_cmp == query_lower
                                } else {
                                    k_cmp.contains(&query_lower)
                                }
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    let matches_value = if target == "values" || target == "both" {
                        let val_str = match &node.value {
                            NodeValue::Str(sid) => Some(self.val_strings.get(*sid).to_string()),
                            NodeValue::Num(nid) => Some(self.nums_pool[*nid as usize].to_string()),
                            NodeValue::Bool(b) => Some(b.to_string()),
                            _ => None,
                        };
                        val_str
                            .map(|v| {
                                let v_cmp = if case_sensitive {
                                    v.clone()
                                } else {
                                    v.to_lowercase()
                                };
                                if exact_match {
                                    v_cmp == query_lower
                                } else {
                                    v_cmp.contains(&query_lower)
                                }
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if matches_key || matches_value {
                        Some((node_id, self.get_path(node_id)))
                    } else {
                        None
                    }
                })
                .collect()
        };

        results.into_iter().take(max_results).collect()
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
            current = self
                .get_children_slice(current)
                .iter()
                .copied()
                .find(|&child_id| {
                    let k = self.nodes[child_id as usize].key;
                    k != u32::MAX && self.keys.get(k) == segment
                })?;
        }

        Some(current)
    }

    fn is_descendant_or_self(&self, node_id: u32, ancestor_id: u32) -> bool {
        let mut current = node_id;
        loop {
            if current == ancestor_id { return true; }
            let p = self.nodes[current as usize].parent;
            if p == u32::MAX { return false; }
            current = p;
        }
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
                if key_case_sensitive && exact_id.is_none() {
                    return None;
                }
                path_segments.push(CompiledPathSegment {
                    raw: segment.to_string(),
                    lower: segment.to_lowercase(),
                    exact_id,
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
            let regex = match filter.operator {
                ObjectSearchOperator::Regex => {
                    let pattern = value.as_ref()?;
                    let mut flags = String::new();
                    if filter.regex_case_insensitive || !value_case_sensitive {
                        flags.push('i');
                    }
                    if filter.regex_multiline { flags.push('m'); }
                    if filter.regex_dot_all { flags.push('s'); }
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
            current = self
                .get_children_slice(current)
                .iter()
                .copied()
                .find(|&child_id| {
                    let child_key_id = self.nodes[child_id as usize].key;
                    if child_key_id == u32::MAX { return false; }
                    if key_case_sensitive {
                        return child_key_id == segment.exact_id.unwrap_or(u32::MAX);
                    }
                    let child_key = self.keys.get(child_key_id);
                    child_key.eq_ignore_ascii_case(&segment.raw)
                        || child_key.to_lowercase() == segment.lower
                })?;
        }
        Some(current)
    }

    fn scalar_value_for_filter(&self, node_id: u32) -> Option<String> {
        match &self.nodes[node_id as usize].value {
            NodeValue::Str(sid) => Some(self.val_strings.get(*sid).to_string()),
            NodeValue::Num(nid) => Some(self.nums_pool[*nid as usize].to_string()),
            NodeValue::Bool(b) => Some(b.to_string()),
            NodeValue::Null => Some("null".to_string()),
            NodeValue::Object | NodeValue::Array => None,
        }
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
            ObjectSearchOperator::Regex => self
                .scalar_value_for_filter(target_id)
                .and_then(|value| filter.regex.as_ref().map(|regex| regex.is_match(&value)))
                .unwrap_or(false),
            ObjectSearchOperator::Contains | ObjectSearchOperator::Equals => self
                .scalar_value_for_filter(target_id)
                .map(|value| {
                    let value_cmp = if value_case_sensitive {
                        value
                    } else {
                        value.to_lowercase()
                    };
                    let needle = filter.value_cmp.as_deref().unwrap_or_default();
                    if filter.operator == ObjectSearchOperator::Equals {
                        value_cmp == needle
                    } else {
                        value_cmp.contains(needle)
                    }
                })
                .unwrap_or(false),
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

        let mut matched_ids: Vec<u32> = self
            .nodes
            .par_iter()
            .enumerate()
            .filter_map(|(idx, node)| {
                let node_id = idx as u32;
                if !matches!(node.value, NodeValue::Object) {
                    return None;
                }
                if let Some(scope_id) = scope_node_id {
                    if !self.is_descendant_or_self(node_id, scope_id) {
                        return None;
                    }
                }
                if compiled_filters.iter().all(|filter| {
                    self.object_filter_matches(
                        node_id,
                        filter,
                        key_case_sensitive,
                        value_case_sensitive,
                    )
                }) {
                    Some(node_id)
                } else {
                    None
                }
            })
            .collect();

        matched_ids
            // node_id == DFS preorder index (invariante del parser streaming)
            .par_sort_unstable();
        matched_ids.truncate(max_results);
        matched_ids
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
                if segment_prefix.is_empty() {
                    true
                } else if candidate.starts_with(segment_prefix) {
                    true
                } else {
                    candidate.to_lowercase().starts_with(&prefix_lower)
                }
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
        let results = index.search("Alice", "values", false, false, false, 10, None, false, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "$.name");
    }

    #[test]
    fn search_by_key() {
        let index = idx(r#"{"username":"bob","email":"b@b.com"}"#);
        let results = index.search("email", "keys", false, false, false, 10, None, false, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "$.email");
    }

    #[test]
    fn search_case_insensitive() {
        let index = idx(r#"{"msg":"Hello World"}"#);
        let results = index.search("hello", "values", false, false, false, 10, None, false, false);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_case_sensitive_no_match() {
        let index = idx(r#"{"msg":"Hello World"}"#);
        let results = index.search("hello", "values", true, false, false, 10, None, false, false);
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
        let exact = index.search("hello", "values", false, false, true, 10, None, false, false);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].1, "$.b");
        let partial = index.search("hello", "values", false, false, false, 10, None, false, false);
        assert_eq!(partial.len(), 3);
    }

    #[test]
    fn search_limited_to_path() {
        let index = idx(r#"{"users":[{"name":"Alice"},{"name":"Bob"}],"meta":{"name":"Catalog"}}"#);
        let results = index.search("name", "keys", false, false, false, 10, Some("$.users.0"), false, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "$.users.0.name");
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
        assert!(matches!(root.value, NodeValue::Str(_)));
    }

    #[test]
    fn scalar_root_number() {
        let index = idx("42");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.value, NodeValue::Num(_)));
    }

    #[test]
    fn scalar_root_bool() {
        let index = idx("true");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.value, NodeValue::Bool(true)));
    }

    #[test]
    fn scalar_root_null() {
        let index = idx("null");
        assert_eq!(index.nodes.len(), 1);
        let root = &index.nodes[index.root as usize];
        assert!(matches!(root.value, NodeValue::Null));
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
        let type_of = |id: u32| match &index.nodes[id as usize].value {
            NodeValue::Str(_) => "string",
            NodeValue::Num(_) => "number",
            NodeValue::Bool(_) => "bool",
            NodeValue::Null => "null",
            NodeValue::Array => "array",
            NodeValue::Object => "object",
        };
        let keys: Vec<&str> = children
            .iter()
            .map(|&id| {
                let k = index.nodes[id as usize].key;
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

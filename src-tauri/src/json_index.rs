use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::File;

// ---- StringPool ----

pub struct StringPool {
    pub strings: Vec<String>,
    map: HashMap<String, u32>,
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
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }

    pub fn get(&self, id: u32) -> &str {
        &self.strings[id as usize]
    }

    pub fn id_of(&self, s: &str) -> Option<u32> {
        self.map.get(s).copied()
    }
}

// ---- NodeValue ----

#[derive(Debug, Clone)]
pub enum NodeValue {
    Object,
    Array,
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

// ---- Node ----

#[derive(Debug, Clone)]
pub struct Node {
    pub id: u32,
    pub parent: Option<u32>,
    pub key: Option<u32>, // index in keys pool
    pub value: NodeValue,
    pub children_start: u32,
    pub children_len: u32,
    pub subtree_len: u32,
    pub preorder_index: u32,
}

// ---- TempNode used during BFS build ----

struct TempNode {
    id: u32,
    parent: Option<u32>,
    key: Option<u32>,
    value: NodeValue,
    children: Vec<u32>,
}

// ---- JsonIndex ----

pub struct JsonIndex {
    pub nodes: Vec<Node>,
    pub children: Vec<u32>,
    pub keys: StringPool,
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
        let value: serde_json::Value = sonic_rs::from_str(json).map_err(|e| e.to_string())?;
        Self::build_index(value)
    }

    /// Carica da file, usando memory-map per file >50MB.
    pub fn from_file(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        let size = metadata.len();
        if size > 50 * 1024 * 1024 {
            let mmap = unsafe { Mmap::map(&file).map_err(|e| e.to_string())? };
            let json = std::str::from_utf8(&mmap).map_err(|e| e.to_string())?;
            Self::from_str(json)
        } else {
            let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Self::from_str(&content)
        }
    }

    /// Parsing streaming da qualsiasi reader (usato per file >200MB con progress callback).
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_reader(reader).map_err(|e| e.to_string())?;
        Self::build_index(value)
    }

    /// Costruisce l'indice con BFS iterativo (no ricorsione).
    fn build_index(root_value: serde_json::Value) -> Result<Self, String> {
        let mut keys = StringPool::new();
        let mut temp_nodes: Vec<TempNode> = Vec::new();

        // BFS queue: (value, parent, key, pre-assigned id)
        // L'ID viene assegnato al momento dell'accodamento, non dell'elaborazione,
        // così tutti i fratelli ricevono ID distinti anche prima di essere processati.
        let mut next_id: u32 = 0;
        let mut queue: VecDeque<(serde_json::Value, Option<u32>, Option<u32>, u32)> =
            VecDeque::new();
        queue.push_back((root_value, None, None, next_id));
        next_id += 1;

        while let Some((value, parent, key, id)) = queue.pop_front() {
            let node_value = match &value {
                serde_json::Value::Object(_) => NodeValue::Object,
                serde_json::Value::Array(_) => NodeValue::Array,
                serde_json::Value::String(s) => NodeValue::Str(s.clone()),
                serde_json::Value::Number(n) => NodeValue::Num(n.as_f64().unwrap_or(0.0)),
                serde_json::Value::Bool(b) => NodeValue::Bool(*b),
                serde_json::Value::Null => NodeValue::Null,
            };

            temp_nodes.push(TempNode {
                id,
                parent,
                key,
                value: node_value,
                children: Vec::new(),
            });

            match value {
                serde_json::Value::Object(map) => {
                    for (k, v) in map {
                        let kid = keys.intern(&k);
                        let child_id = next_id;
                        next_id += 1;
                        temp_nodes[id as usize].children.push(child_id);
                        queue.push_back((v, Some(id), Some(kid), child_id));
                    }
                }
                serde_json::Value::Array(arr) => {
                    for (i, v) in arr.into_iter().enumerate() {
                        let kid = keys.intern(&i.to_string());
                        let child_id = next_id;
                        next_id += 1;
                        temp_nodes[id as usize].children.push(child_id);
                        queue.push_back((v, Some(id), Some(kid), child_id));
                    }
                }
                _ => {}
            }
        }

        // Flatten temp_nodes into flat children arena
        let total_children: usize = temp_nodes.iter().map(|n| n.children.len()).sum();
        let mut children: Vec<u32> = Vec::with_capacity(total_children);
        let mut nodes: Vec<Node> = Vec::with_capacity(temp_nodes.len());

        for tn in temp_nodes {
            let children_start = children.len() as u32;
            let children_len = tn.children.len() as u32;
            children.extend_from_slice(&tn.children);
            nodes.push(Node {
                id: tn.id,
                parent: tn.parent,
                key: tn.key,
                value: tn.value,
                children_start,
                children_len,
                subtree_len: 1,
                preorder_index: 0,
            });
        }

        let mut next_preorder_index = 0u32;
        let mut stack = vec![0u32];
        while let Some(node_id) = stack.pop() {
            nodes[node_id as usize].preorder_index = next_preorder_index;
            next_preorder_index += 1;

            let node = &nodes[node_id as usize];
            let start = node.children_start as usize;
            let end = start + node.children_len as usize;
            for &child_id in children[start..end].iter().rev() {
                stack.push(child_id);
            }
        }

        for idx in (0..nodes.len()).rev() {
            let child_ids: Vec<u32> = {
                let node = &nodes[idx];
                let start = node.children_start as usize;
                let end = start + node.children_len as usize;
                children[start..end].to_vec()
            };
            let subtree_len = 1 + child_ids
                .iter()
                .map(|&child_id| nodes[child_id as usize].subtree_len)
                .sum::<u32>();
            nodes[idx].subtree_len = subtree_len;
        }

        Ok(JsonIndex {
            nodes,
            children,
            keys,
            root: 0,
        })
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
                                    if let Some(kid) = child_node.key {
                                        stack.push(Task::Key(kid));
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
                        NodeValue::Str(s) => {
                            out.push('"');
                            json_escape_into(&mut out, s);
                            out.push('"');
                        }
                        NodeValue::Num(n) => {
                            let f = *n;
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
            if let Some(kid) = node.key {
                key_ids.push(kid);
            }
            match node.parent {
                Some(p) => current = p,
                None => break,
            }
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
                .filter_map(|node| {
                    if let Some(scope_id) = scope_node_id {
                        if !self.is_descendant_or_self(node.id, scope_id) {
                            return None;
                        }
                    }

                    let key_str: Option<&str> = node.key.map(|kid| self.keys.get(kid));
                    let matches_key = (target == "keys" || target == "both")
                        && key_str.map(|k| re.is_match(k)).unwrap_or(false);

                    let matches_value = (target == "values" || target == "both")
                        && match &node.value {
                            NodeValue::Str(s) => re.is_match(s),
                            NodeValue::Num(n) => re.is_match(&n.to_string()),
                            NodeValue::Bool(b) => re.is_match(&b.to_string()),
                            _ => false,
                        };

                    if matches_key || matches_value {
                        Some((node.id, self.get_path(node.id)))
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
                .filter_map(|node| {
                    if let Some(scope_id) = scope_node_id {
                        if !self.is_descendant_or_self(node.id, scope_id) {
                            return None;
                        }
                    }

                    let key_str: Option<&str> = node.key.map(|kid| self.keys.get(kid));
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
                            NodeValue::Str(s) => Some(s.as_str().to_string()),
                            NodeValue::Num(n) => Some(n.to_string()),
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
                        Some((node.id, self.get_path(node.id)))
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
                    self.nodes[child_id as usize]
                        .key
                        .map(|kid| self.keys.get(kid) == segment)
                        .unwrap_or(false)
                })?;
        }

        Some(current)
    }

    fn is_descendant_or_self(&self, node_id: u32, ancestor_id: u32) -> bool {
        let mut current = Some(node_id);
        while let Some(id) = current {
            if id == ancestor_id {
                return true;
            }
            current = self.nodes[id as usize].parent;
        }
        false
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
                    let Some(child_key_id) = self.nodes[child_id as usize].key else {
                        return false;
                    };
                    if key_case_sensitive {
                        return Some(child_key_id) == segment.exact_id;
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
            NodeValue::Str(s) => Some(s.clone()),
            NodeValue::Num(n) => Some(n.to_string()),
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
            .filter_map(|node| {
                if !matches!(node.value, NodeValue::Object) {
                    return None;
                }
                if let Some(scope_id) = scope_node_id {
                    if !self.is_descendant_or_self(node.id, scope_id) {
                        return None;
                    }
                }
                if compiled_filters.iter().all(|filter| {
                    self.object_filter_matches(
                        node.id,
                        filter,
                        key_case_sensitive,
                        value_case_sensitive,
                    )
                }) {
                    Some(node.id)
                } else {
                    None
                }
            })
            .collect();

        matched_ids
            .par_sort_unstable_by_key(|&node_id| self.nodes[node_id as usize].preorder_index);
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

        let mut suggestions: Vec<&str> = self
            .keys
            .strings
            .iter()
            .map(String::as_str)
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

    fn normalize(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
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
    fn preorder_index_matches_file_order() {
        let index = idx(r#"{"a":{"x":1},"b":2}"#);
        let a_id = index.get_children_slice(index.root)[0];
        let b_id = index.get_children_slice(index.root)[1];
        let x_id = index.get_children_slice(a_id)[0];

        assert!(
            index.nodes[a_id as usize].preorder_index < index.nodes[x_id as usize].preorder_index
        );
        assert!(
            index.nodes[x_id as usize].preorder_index < index.nodes[b_id as usize].preorder_index
        );
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
            .map(|&id| index.keys.get(index.nodes[id as usize].key.unwrap()))
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
        assert_eq!(
            index
                .keys
                .strings
                .iter()
                .filter(|s| s.as_str() == "name")
                .count(),
            1
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

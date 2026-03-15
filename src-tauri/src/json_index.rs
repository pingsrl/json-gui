use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use sonic_rs;
use std::fs::File;

#[derive(Debug, Clone)]
pub enum NodeValue {
    Object,
    Array,
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: u32,
    pub parent: Option<u32>,
    pub key: Option<String>,
    pub value: NodeValue,
    pub children: Vec<u32>,
}

pub struct JsonIndex {
    pub nodes: Vec<Node>,
    pub root: u32,
}

impl JsonIndex {
    pub fn from_str(json: &str) -> Result<Self, String> {
        let value: serde_json::Value = sonic_rs::from_str(json)
            .map_err(|e| e.to_string())?;

        let mut nodes: Vec<Node> = Vec::new();
        let root = Self::build_tree(&value, None, None, &mut nodes);
        Ok(JsonIndex { nodes, root })
    }

    /// Carica da file, usando memory-map per file 50-200MB, streaming per file >200MB.
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
        let mut nodes: Vec<Node> = Vec::new();
        let root = Self::build_tree(&value, None, None, &mut nodes);
        Ok(JsonIndex { nodes, root })
    }

    fn build_tree(
        value: &serde_json::Value,
        parent: Option<u32>,
        key: Option<String>,
        nodes: &mut Vec<Node>,
    ) -> u32 {
        let id = nodes.len() as u32;

        let node_value = match value {
            serde_json::Value::Object(_) => NodeValue::Object,
            serde_json::Value::Array(_) => NodeValue::Array,
            serde_json::Value::String(s) => NodeValue::Str(s.clone()),
            serde_json::Value::Number(n) => NodeValue::Num(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::Bool(b) => NodeValue::Bool(*b),
            serde_json::Value::Null => NodeValue::Null,
        };

        nodes.push(Node {
            id,
            parent,
            key,
            value: node_value,
            children: Vec::new(),
        });

        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let child_id = Self::build_tree(v, Some(id), Some(k.clone()), nodes);
                    nodes[id as usize].children.push(child_id);
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, v) in arr.iter().enumerate() {
                    let child_id = Self::build_tree(v, Some(id), Some(i.to_string()), nodes);
                    nodes[id as usize].children.push(child_id);
                }
            }
            _ => {}
        }

        id
    }

    pub fn search(
        &self,
        query: &str,
        target: &str,
        case_sensitive: bool,
        use_regex: bool,
        max_results: usize,
    ) -> Vec<(u32, String)> {
        let results: Vec<(u32, String)> = if use_regex {
            let pattern = if case_sensitive {
                query.to_string()
            } else {
                format!("(?i){}", query)
            };
            let re = match Regex::new(&pattern) {
                Ok(r) => r,
                Err(_) => return vec![],
            };
            self.nodes
                .par_iter()
                .filter_map(|node| {
                    let matches_key = (target == "keys" || target == "both")
                        && node.key.as_ref().map(|k| re.is_match(k)).unwrap_or(false);

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
                    let matches_key = if target == "keys" || target == "both" {
                        node.key
                            .as_ref()
                            .map(|k| {
                                let k_cmp = if case_sensitive {
                                    k.clone()
                                } else {
                                    k.to_lowercase()
                                };
                                k_cmp.contains(&query_lower)
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    let matches_value = if target == "values" || target == "both" {
                        let val_str = match &node.value {
                            NodeValue::Str(s) => Some(s.clone()),
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
                                v_cmp.contains(&query_lower)
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

    pub fn get_path(&self, node_id: u32) -> String {
        let mut parts = Vec::new();
        let mut current = node_id;
        loop {
            let node = &self.nodes[current as usize];
            if let Some(key) = &node.key {
                parts.push(key.clone());
            }
            match node.parent {
                Some(p) => current = p,
                None => break,
            }
        }
        parts.reverse();
        if parts.is_empty() {
            "$".to_string()
        } else {
            format!("$.{}", parts.join("."))
        }
    }
}

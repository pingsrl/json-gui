import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export interface NodeDto {
  id: number;
  key: string | null;
  value_type: string;
  value_preview: string;
  children_count: number;
}

export interface VNode {
  node: NodeDto;
  depth: number;
}

export interface SearchResult {
  node_id: number;
  file_order: number;
  path: string;
  key: string | null;
  value_preview: string;
  kind: "node" | "object";
  match_preview?: string | null;
}

export type SearchSortMode = "relevance" | "file";
export type SearchMode = "text" | "object";

export interface ObjectSearchFilter {
  path: string;
  operator: "contains" | "equals" | "regex" | "exists";
  value?: string;
  regexCaseInsensitive?: boolean;
  regexMultiline?: boolean;
  regexDotAll?: boolean;
}

interface FileInfo {
  node_count: number;
  size_bytes: number;
  root_node: NodeDto;
  root_children: NodeDto[];
}

interface ExpandToResult {
  expansions: [number, NodeDto[]][];
  path: string;
}

const MAX_RECENT = 5;
const MAX_CHILDREN_CACHE = 2000;

// Cache LRU semplice (FIFO con limite) per i figli già caricati
const childrenCache = new Map<number, NodeDto[]>();

// Cache a livello modulo (non reactive) per path e nodi noti
const pathCache = new Map<number, string>();
const nodeMapCache = new Map<number, NodeDto>();
// Mappa figlio→genitore per O(1) sibling lookup
const parentMap = new Map<number, number>();

// Lookup O(1) del genitore di un nodo (usato da TreePanel per ArrowLeft)
export function getParentId(nodeId: number): number | undefined {
  return parentMap.get(nodeId);
}

function registerChildren(parentId: number, children: NodeDto[]) {
  for (const c of children) parentMap.set(c.id, parentId);
}

function findSiblings(
  nodeId: number,
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>
): NodeDto[] | null {
  if (rootChildren.some((c) => c.id === nodeId)) return rootChildren;
  const pid = parentMap.get(nodeId);
  if (pid !== undefined) return expandedNodes.get(pid) ?? null;
  return null;
}

function cacheSet(id: number, children: NodeDto[]) {
  if (childrenCache.size >= MAX_CHILDREN_CACHE) {
    const firstKey = childrenCache.keys().next().value;
    if (firstKey !== undefined) childrenCache.delete(firstKey);
  }
  childrenCache.set(id, children);
}

function loadRecentFiles(): string[] {
  try {
    return JSON.parse(localStorage.getItem("recentFiles") ?? "[]");
  } catch {
    return [];
  }
}


export function buildVisibleNodes(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>
): VNode[] {
  const result: VNode[] = [];
  // Stack iterativo: evita stack overflow su alberi profondi
  const stack: Array<{ nodes: NodeDto[]; depth: number; index: number }> = [
    { nodes: rootChildren, depth: 0, index: 0 }
  ];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame.index >= frame.nodes.length) {
      stack.pop();
      continue;
    }
    const node = frame.nodes[frame.index++];
    result.push({ node, depth: frame.depth });
    const children = expandedNodes.get(node.id);
    if (children && children.length > 0) {
      stack.push({ nodes: children, depth: frame.depth + 1, index: 0 });
    }
  }
  return result;
}

export function insertVisibleChildren(
  visibleNodes: VNode[],
  expansions: [number, NodeDto[]][]
): VNode[] {
  if (expansions.length === 0) return visibleNodes;

  const nextVisible = visibleNodes.slice();
  for (const [parentId, children] of expansions) {
    if (children.length === 0) continue;
    const parentIndex = nextVisible.findIndex(
      ({ node }) => node.id === parentId
    );
    if (parentIndex < 0) continue;
    const depth = nextVisible[parentIndex].depth + 1;
    nextVisible.splice(
      parentIndex + 1,
      0,
      ...children.map((node) => ({ node, depth }))
    );
  }
  return nextVisible;
}

export interface ContextMenuState {
  x: number;
  y: number;
  nodeId: number;
  parentId: number | null;
  nodeKey: string | null;
  valueType: string;
}

function getSearchRelevanceScore(result: SearchResult, query: string): number {
  const key = result.key ?? "";
  const value = result.value_preview;
  const queryLower = query.toLowerCase();
  const keyLower = key.toLowerCase();
  const valueLower = value.toLowerCase();

  if (key === query || value === query) return 0;
  if (keyLower === queryLower || valueLower === queryLower) return 1;
  if (key.startsWith(query) || value.startsWith(query)) return 2;
  if (keyLower.startsWith(queryLower) || valueLower.startsWith(queryLower))
    return 3;
  return 4;
}

export function sortSearchResults(
  results: SearchResult[],
  query: string,
  sortMode: SearchSortMode
): SearchResult[] {
  if (results.length === 0) return results;
  const sorted = results.slice();

  if (sortMode === "file") {
    sorted.sort((a, b) => a.file_order - b.file_order);
    return sorted;
  }

  // Pre-calcola gli score una sola volta: da O(n·log(n)·k) a O(n + n·log(n))
  const scores = new Map<SearchResult, number>();
  for (const r of sorted) {
    if (r.kind !== "object") scores.set(r, getSearchRelevanceScore(r, query));
  }
  sorted.sort((a, b) => {
    if (a.kind === "object" || b.kind === "object") {
      return a.file_order - b.file_order;
    }
    const relevanceDelta = (scores.get(a) ?? 4) - (scores.get(b) ?? 4);
    if (relevanceDelta !== 0) return relevanceDelta;
    return a.file_order - b.file_order;
  });
  return sorted;
}

interface JsonStore {
  filePath: string | null;
  nodeCount: number;
  sizeBytes: number;
  rootNode: NodeDto | null;
  rootChildren: NodeDto[];
  expandedNodes: Map<number, NodeDto[]>;
  expandProgress: number | null;
  selectedNodeId: number | null;
  selectedNode: NodeDto | null;
  selectedNodePath: string | null;
  focusedNodeId: number | null;
  visibleNodes: VNode[];
  selectedNodeSiblings: NodeDto[] | null;
  recentFiles: string[];
  searchMode: SearchMode;
  activeSearchMode: SearchMode | null;
  hasActiveSearch: boolean;
  searchScopePath: string;
  searchSort: SearchSortMode;
  lastSearchQuery: string;
  searchResults: SearchResult[];
  searching: boolean;
  loading: boolean;
  contextMenu: ContextMenuState | null;
  openFile: (path: string) => Promise<void>;
  openFromString: (content: string) => Promise<void>;
  toggleNode: (nodeId: number) => Promise<void>;
  selectNode: (node: NodeDto) => Promise<void>;
  navigateToNode: (nodeId: number) => Promise<void>;
  setFocusedNode: (nodeId: number | null) => void;
  search: (
    query: string,
    target: string,
    caseSensitive: boolean,
    useRegex: boolean,
    exactMatch: boolean,
    path: string,
    multiline?: boolean,
    dotAll?: boolean
  ) => Promise<void>;
  searchObjects: (
    filters: ObjectSearchFilter[],
    keyCaseSensitive: boolean,
    valueCaseSensitive: boolean,
    path: string
  ) => Promise<void>;
  setSearchMode: (mode: SearchMode) => void;
  setSearchScopePath: (path: string) => void;
  setSearchSort: (sortMode: SearchSortMode) => void;
  expandAll: () => Promise<void>;
  expandSubtree: (nodeId: number) => Promise<void>;
  collapseAll: () => void;
  clearSearch: () => void;
  showContextMenu: (cm: ContextMenuState) => void;
  hideContextMenu: () => void;
}

export const useJsonStore = create<JsonStore>((set, get) => ({
  filePath: null,
  nodeCount: 0,
  sizeBytes: 0,
  rootNode: null,
  rootChildren: [],
  expandedNodes: new Map(),
  expandProgress: null,
  selectedNodeId: null,
  selectedNode: null,
  selectedNodePath: null,
  focusedNodeId: null,
  visibleNodes: [],
  selectedNodeSiblings: null,
  recentFiles: loadRecentFiles(),
  searchMode: "text",
  activeSearchMode: null,
  hasActiveSearch: false,
  searchScopePath: "",
  searchSort: "relevance",
  lastSearchQuery: "",
  searchResults: [],
  searching: false,
  loading: false,
  contextMenu: null,

  openFile: async (path: string) => {
    set({ loading: true });
    const info = await invoke<FileInfo>("open_file", { path }).finally(() =>
      set({ loading: false })
    );
    const expandedNodes = new Map<number, NodeDto[]>();
    const visibleNodes = buildVisibleNodes(info.root_children, expandedNodes);

    childrenCache.clear();
    pathCache.clear();
    nodeMapCache.clear();
    parentMap.clear();

    // Popola nodeMapCache con root_children
    for (const n of info.root_children) {
      nodeMapCache.set(n.id, n);
    }

    const prev = get().recentFiles.filter((f) => f !== path);
    const recentFiles = [path, ...prev].slice(0, MAX_RECENT);
    localStorage.setItem("recentFiles", JSON.stringify(recentFiles));

    set({
      filePath: path,
      nodeCount: info.node_count,
      sizeBytes: info.size_bytes,
      rootNode: info.root_node,
      rootChildren: info.root_children,
      expandedNodes,
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
      recentFiles,
      activeSearchMode: null,
      hasActiveSearch: false,
      searchScopePath: "",
      lastSearchQuery: "",
      searchResults: []
    });
  },

  openFromString: async (content: string) => {
    set({ loading: true });
    const info = await invoke<FileInfo>("open_from_string", {
      content
    }).finally(() => set({ loading: false }));
    const expandedNodes = new Map<number, NodeDto[]>();
    const visibleNodes = buildVisibleNodes(info.root_children, expandedNodes);

    childrenCache.clear();
    pathCache.clear();
    nodeMapCache.clear();
    parentMap.clear();

    for (const n of info.root_children) {
      nodeMapCache.set(n.id, n);
    }

    set({
      filePath: "(incollato)",
      nodeCount: info.node_count,
      sizeBytes: info.size_bytes,
      rootNode: info.root_node,
      rootChildren: info.root_children,
      expandedNodes,
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
      activeSearchMode: null,
      hasActiveSearch: false,
      searchScopePath: "",
      lastSearchQuery: "",
      searchResults: []
    });
  },

  toggleNode: async (nodeId: number) => {
    const { expandedNodes, rootChildren, selectedNodeId } = get();
    let next: Map<number, NodeDto[]>;
    if (expandedNodes.has(nodeId)) {
      next = new Map(expandedNodes);
      next.delete(nodeId);
    } else {
      let children = childrenCache.get(nodeId);
      if (!children) {
        children = await invoke<NodeDto[]>("get_children", { nodeId });
        cacheSet(nodeId, children);
      }
      for (const child of children) {
        nodeMapCache.set(child.id, child);
      }
      registerChildren(nodeId, children);
      next = new Map(expandedNodes);
      next.set(nodeId, children);
    }
    const visibleNodes = buildVisibleNodes(rootChildren, next);
    const selectedNodeSiblings =
      selectedNodeId !== null
        ? findSiblings(selectedNodeId, rootChildren, next)
        : null;
    set({ expandedNodes: next, visibleNodes, selectedNodeSiblings });
  },

  selectNode: async (node: NodeDto) => {
    const { rootChildren, expandedNodes } = get();
    // parent_id è stato rimosso da NodeDto; usiamo parentMap (popolata da registerChildren).
    const selectedNodeSiblings = findSiblings(node.id, rootChildren, expandedNodes);
    const cachedPath = pathCache.get(node.id) ?? null;
    set({
      selectedNodeId: node.id,
      selectedNode: node,
      selectedNodePath: cachedPath,
      selectedNodeSiblings,
      focusedNodeId: node.id
    });
    if (cachedPath) return;
    try {
      const path = await invoke<string>("get_path", { nodeId: node.id });
      pathCache.set(node.id, path);
      if (get().selectedNodeId === node.id) {
        set({ selectedNodePath: path });
      }
    } catch (err) {
      console.error("selectNode path error:", err);
    }
  },

  setFocusedNode: (nodeId: number | null) => {
    set({ focusedNodeId: nodeId });
  },

  search: async (
    query: string,
    target: string,
    caseSensitive: boolean,
    useRegex: boolean,
    exactMatch: boolean,
    path: string,
    multiline = false,
    dotAll = false
  ) => {
    if (!query.trim()) {
      get().clearSearch();
      return;
    }
    set({
      searching: true,
      selectedNode: null,
      selectedNodeId: null,
      activeSearchMode: "text",
      hasActiveSearch: true
    });
    try {
      const results = await invoke<SearchResult[]>("search", {
        query: {
          text: query,
          target,
          case_sensitive: caseSensitive,
          regex: useRegex,
          exact_match: exactMatch,
          max_results: 500,
          path: path.trim() || null,
          multiline,
          dot_all: dotAll
        }
      });
      const sortedResults = sortSearchResults(results, query, get().searchSort);
      set({
        searchResults: sortedResults,
        searching: false,
        lastSearchQuery: query,
        activeSearchMode: "text",
        hasActiveSearch: true
      });
    } catch (err) {
      console.error("Search error:", err);
      set({
        searching: false,
        activeSearchMode: "text",
        hasActiveSearch: true
      });
    }
  },

  searchObjects: async (
    filters: ObjectSearchFilter[],
    keyCaseSensitive: boolean,
    valueCaseSensitive: boolean,
    path: string
  ) => {
    if (filters.length === 0) {
      get().clearSearch();
      return;
    }
    set({
      searching: true,
      selectedNode: null,
      selectedNodeId: null,
      activeSearchMode: "object",
      hasActiveSearch: true
    });
    try {
      const results = await invoke<SearchResult[]>("search_objects", {
        query: {
          filters,
          key_case_sensitive: keyCaseSensitive,
          value_case_sensitive: valueCaseSensitive,
          max_results: 500,
          path: path.trim() || null
        }
      });
      set({
        searchResults: sortSearchResults(results, "", "file"),
        searching: false,
        lastSearchQuery: "",
        activeSearchMode: "object",
        hasActiveSearch: true
      });
    } catch (err) {
      console.error("Object search error:", err);
      set({
        searching: false,
        activeSearchMode: "object",
        hasActiveSearch: true
      });
    }
  },

  setSearchMode: (searchMode: SearchMode) => set({ searchMode }),

  setSearchScopePath: (path: string) => {
    set({ searchScopePath: path });
  },

  setSearchSort: (searchSort: SearchSortMode) => {
    const { searchResults, lastSearchQuery } = get();
    set({
      searchSort,
      searchResults: sortSearchResults(
        searchResults,
        lastSearchQuery,
        searchSort
      )
    });
  },

  navigateToNode: async (nodeId: number) => {
    const result = await invoke<ExpandToResult>("expand_to", { nodeId });
    const { expandedNodes, rootChildren } = get();
    const next = new Map(expandedNodes);
    for (const [id, children] of result.expansions) {
      next.set(id, children);
      cacheSet(id, children);
      registerChildren(id, children);
      for (const child of children) {
        nodeMapCache.set(child.id, child);
      }
    }
    pathCache.set(nodeId, result.path);

    const visibleNodes = buildVisibleNodes(rootChildren, next);
    const targetNode = nodeMapCache.get(nodeId) ?? null;
    const selectedNodeSiblings = findSiblings(nodeId, rootChildren, next);
    set({
      expandedNodes: next,
      selectedNodeId: nodeId,
      selectedNode: targetNode,
      selectedNodePath: result.path,
      selectedNodeSiblings,
      focusedNodeId: nodeId,
      visibleNodes
    });
  },

  expandAll: async () => {
    const { rootNode, rootChildren } = get();
    if (rootChildren.length === 0 || !rootNode) return;
    set({ loading: true });
    try {
      const expansions = await invoke<[number, NodeDto[]][]>("expand_subtree", {
        nodeId: rootNode.id,
        maxNodes: 1_000_000
      });
      const next = new Map<number, NodeDto[]>();
      for (const [parentId, children] of expansions) {
        next.set(parentId, children);
        cacheSet(parentId, children);
        registerChildren(parentId, children);
        for (const child of children) nodeMapCache.set(child.id, child);
      }
      const visibleNodes = buildVisibleNodes(rootChildren, next);
      set({ expandedNodes: next, visibleNodes });
    } catch (err) {
      console.error("expandAll failed:", err);
    } finally {
      set({ loading: false });
    }
  },

  expandSubtree: async (nodeId: number) => {
    const { expandedNodes, rootChildren } = get();
    const expansions = await invoke<[number, NodeDto[]][]>("expand_subtree", {
      nodeId
    });
    const next = new Map(expandedNodes);
    for (const [parentId, children] of expansions) {
      next.set(parentId, children);
      cacheSet(parentId, children);
      registerChildren(parentId, children);
      for (const child of children) {
        nodeMapCache.set(child.id, child);
      }
    }
    const visibleNodes = buildVisibleNodes(rootChildren, next);
    set({ expandedNodes: next, visibleNodes });
  },

  collapseAll: () => {
    const { rootChildren, selectedNodeId } = get();
    const next = new Map<number, NodeDto[]>();
    const visibleNodes = buildVisibleNodes(rootChildren, next);
    const selectedNodeSiblings =
      selectedNodeId !== null
        ? findSiblings(selectedNodeId, rootChildren, next)
        : null;
    set({ expandedNodes: next, visibleNodes, selectedNodeSiblings });
  },

  clearSearch: () =>
    set({
      searchResults: [],
      searching: false,
      lastSearchQuery: "",
      activeSearchMode: null,
      hasActiveSearch: false
    }),

  showContextMenu: (cm: ContextMenuState) => set({ contextMenu: cm }),
  hideContextMenu: () => set({ contextMenu: null })
}));

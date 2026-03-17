import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export interface NodeDto {
  id: number;
  parent_id?: number | null;
  key: string | null;
  value_type: string;
  value_preview: string;
  has_children: boolean;
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
}

export type SearchSortMode = "relevance" | "file";

interface FileInfo {
  node_count: number;
  size_bytes: number;
  root_children: NodeDto[];
}

interface ExpandToResult {
  expansions: [number, NodeDto[]][];
  path: string;
}

interface ExpandedSliceResult {
  offset: number;
  total_count: number;
  rows: VNode[];
}

const MAX_RECENT = 5;
// Contatore generazione per ignorare chunk di expand_all superati da un reload
let expandGeneration = 0;
const MAX_CHILDREN_CACHE = 2000;
const EXPAND_ALL_SLICE_SIZE = 200;

const expandAllRequestedPages = new Set<number>();

// Cache LRU semplice (FIFO con limite) per i figli già caricati
const childrenCache = new Map<number, NodeDto[]>();

// Cache a livello modulo (non reactive) per path e nodi noti
const pathCache = new Map<number, string>();
const nodeMapCache = new Map<number, NodeDto>();
// Mappa figlio→genitore per O(1) sibling lookup
const parentMap = new Map<number, number>();

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

function clearExpandAllRequests() {
  expandAllRequestedPages.clear();
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
    const parentIndex = nextVisible.findIndex(({ node }) => node.id === parentId);
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
  valueType: string;
  valuePreview: string;
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
  const sorted = results.slice();
  sorted.sort((a, b) => {
    if (sortMode === "file") {
      return a.file_order - b.file_order;
    }
    const relevanceDelta =
      getSearchRelevanceScore(a, query) - getSearchRelevanceScore(b, query);
    if (relevanceDelta !== 0) return relevanceDelta;
    return a.file_order - b.file_order;
  });
  return sorted;
}

interface JsonStore {
  filePath: string | null;
  nodeCount: number;
  sizeBytes: number;
  rootChildren: NodeDto[];
  expandedNodes: Map<number, NodeDto[]>;
  expandAllActive: boolean;
  expandAllTotalCount: number;
  expandAllRows: Map<number, VNode>;
  expandProgress: number | null;
  selectedNodeId: number | null;
  selectedNode: NodeDto | null;
  selectedNodePath: string | null;
  focusedNodeId: number | null;
  visibleNodes: VNode[];
  selectedNodeSiblings: NodeDto[] | null;
  recentFiles: string[];
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
    path: string
  ) => Promise<void>;
  setSearchScopePath: (path: string) => void;
  setSearchSort: (sortMode: SearchSortMode) => void;
  expandAll: () => Promise<void>;
  fetchExpandedSlice: (offset: number, limit: number) => Promise<void>;
  collapseAll: () => void;
  clearSearch: () => void;
  showContextMenu: (cm: ContextMenuState) => void;
  hideContextMenu: () => void;
}

export const useJsonStore = create<JsonStore>((set, get) => ({
  filePath: null,
  nodeCount: 0,
  sizeBytes: 0,
  rootChildren: [],
  expandedNodes: new Map(),
  expandAllActive: false,
  expandAllTotalCount: 0,
  expandAllRows: new Map(),
  expandProgress: null,
  selectedNodeId: null,
  selectedNode: null,
  selectedNodePath: null,
  focusedNodeId: null,
  visibleNodes: [],
  selectedNodeSiblings: null,
  recentFiles: loadRecentFiles(),
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
    clearExpandAllRequests();

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
      rootChildren: info.root_children,
      expandedNodes,
      expandAllActive: false,
      expandAllTotalCount: 0,
      expandAllRows: new Map(),
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
      recentFiles,
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
    clearExpandAllRequests();

    for (const n of info.root_children) {
      nodeMapCache.set(n.id, n);
    }

    set({
      filePath: "(incollato)",
      nodeCount: info.node_count,
      sizeBytes: info.size_bytes,
      rootChildren: info.root_children,
      expandedNodes,
      expandAllActive: false,
      expandAllTotalCount: 0,
      expandAllRows: new Map(),
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
      searchScopePath: "",
      lastSearchQuery: "",
      searchResults: []
    });
  },

  toggleNode: async (nodeId: number) => {
    const { expandAllActive, expandedNodes, rootChildren, selectedNodeId } = get();
    if (expandAllActive) return;
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
    const { expandAllActive, rootChildren, expandedNodes } = get();
    const selectedNodeSiblings = expandAllActive
      ? null
      : findSiblings(node.id, rootChildren, expandedNodes);
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
    path: string
  ) => {
    if (!query.trim()) {
      get().clearSearch();
      return;
    }
    set({ searching: true, selectedNode: null, selectedNodeId: null });
    try {
      const results = await invoke<SearchResult[]>("search", {
        query: {
          text: query,
          target,
          case_sensitive: caseSensitive,
          regex: useRegex,
          exact_match: exactMatch,
          max_results: 500,
          path: path.trim() || null
        }
      });
      const sortedResults = sortSearchResults(results, query, get().searchSort);
      set({
        searchResults: sortedResults,
        searching: false,
        lastSearchQuery: query
      });
    } catch (err) {
      console.error("Search error:", err);
      set({ searching: false });
    }
  },

  setSearchScopePath: (path: string) => {
    set({ searchScopePath: path });
  },

  setSearchSort: (searchSort: SearchSortMode) => {
    const { searchResults, lastSearchQuery } = get();
    set({
      searchSort,
      searchResults: sortSearchResults(searchResults, lastSearchQuery, searchSort)
    });
  },

  navigateToNode: async (nodeId: number) => {
    clearExpandAllRequests();
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
      expandAllActive: false,
      expandAllTotalCount: 0,
      expandAllRows: new Map(),
      selectedNodeId: nodeId,
      selectedNode: targetNode,
      selectedNodePath: result.path,
      selectedNodeSiblings,
      focusedNodeId: nodeId,
      visibleNodes
    });
  },

  fetchExpandedSlice: async (offset: number, limit: number) => {
    const { expandAllActive } = get();
    if (!expandAllActive) return;

    const gen = expandGeneration;
    const start =
      Math.floor(Math.max(0, offset) / EXPAND_ALL_SLICE_SIZE) * EXPAND_ALL_SLICE_SIZE;
    const end = Math.max(
      start + EXPAND_ALL_SLICE_SIZE,
      Math.ceil((Math.max(0, offset) + Math.max(1, limit)) / EXPAND_ALL_SLICE_SIZE) *
        EXPAND_ALL_SLICE_SIZE
    );

    const tasks: Promise<void>[] = [];
    for (let page = start; page < end; page += EXPAND_ALL_SLICE_SIZE) {
      if (expandAllRequestedPages.has(page)) continue;
      expandAllRequestedPages.add(page);
      tasks.push(
        invoke<ExpandedSliceResult>("get_expanded_slice", {
          offset: page,
          limit: EXPAND_ALL_SLICE_SIZE
        })
          .then((result) => {
            if (expandGeneration !== gen || !get().expandAllActive) return;
            const nextRows = new Map(get().expandAllRows);
            result.rows.forEach((row, idx) => {
              nextRows.set(result.offset + idx, row);
            });
            set({
              expandAllRows: nextRows,
              expandAllTotalCount: result.total_count
            });
          })
          .catch((err) => {
            console.error("get_expanded_slice failed:", err);
            expandAllRequestedPages.delete(page);
          })
      );
    }

    await Promise.all(tasks);
  },

  expandAll: async () => {
    const { rootChildren, nodeCount } = get();
    if (rootChildren.length === 0) return;

    expandGeneration++;
    const myGen = expandGeneration;
    clearExpandAllRequests();
    set({
      loading: true,
      expandProgress: null,
      expandedNodes: new Map(),
      expandAllActive: true,
      expandAllTotalCount: Math.max(0, nodeCount - 1),
      expandAllRows: new Map(),
      selectedNodeSiblings: null
    });
    try {
      await get().fetchExpandedSlice(0, EXPAND_ALL_SLICE_SIZE * 2);
    } catch (err) {
      console.error("expandAll failed:", err);
    }
    if (expandGeneration === myGen) {
      set({ loading: false, expandProgress: null });
    }
  },

  collapseAll: () => {
    clearExpandAllRequests();
    const { rootChildren, selectedNodeId } = get();
    const next = new Map<number, NodeDto[]>();
    const visibleNodes = buildVisibleNodes(rootChildren, next);
    const selectedNodeSiblings =
      selectedNodeId !== null
        ? findSiblings(selectedNodeId, rootChildren, next)
        : null;
    set({
      expandedNodes: next,
      expandAllActive: false,
      expandAllTotalCount: 0,
      expandAllRows: new Map(),
      visibleNodes,
      selectedNodeSiblings
    });
  },

  clearSearch: () =>
    set({ searchResults: [], searching: false, lastSearchQuery: "" }),

  showContextMenu: (cm: ContextMenuState) => set({ contextMenu: cm }),
  hideContextMenu: () => set({ contextMenu: null })
}));

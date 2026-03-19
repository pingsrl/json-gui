import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export interface NodeDto {
  id: number;
  key: string | null;
  value_type: string;
  value_preview: string;
  children_count: number;
  synthetic_kind?: "load-more";
  parent_node_id?: number;
  next_offset?: number;
  remaining_count?: number;
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

export interface RuntimeStats {
  resident_bytes: number;
  cpu_percent: number;
  sampled_at_ms: number;
}

export interface OperationMetrics {
  label: string;
  duration_ms: number;
  cpu_percent: number | null;
  resident_bytes: number | null;
  finished_at_ms: number;
}

const MAX_RECENT = 5;
const MAX_CHILDREN_CACHE = 2000;
const LARGE_NODE_PAGE_SIZE = 1000;
const MAX_SAFE_EXPAND_NODES = 50_000;

// Cache LRU semplice (FIFO con limite) per i figli già caricati
const childrenCache = new Map<number, NodeDto[]>();
const childrenLoadInFlight = new Map<number, Promise<NodeDto[]>>();
const childrenPageLoadInFlight = new Map<string, Promise<NodeDto[]>>();
const desiredExpandedState = new Map<number, boolean>();
let nextSyntheticNodeId = -1;

// Cache a livello modulo (non reactive) per path e nodi noti
const pathCache = new Map<number, string>();
const nodeMapCache = new Map<number, NodeDto>();
// Mappa figlio→genitore per O(1) sibling lookup
const parentMap = new Map<number, number>();

function resetTreeCaches() {
  childrenCache.clear();
  childrenLoadInFlight.clear();
  childrenPageLoadInFlight.clear();
  desiredExpandedState.clear();
  pathCache.clear();
  nodeMapCache.clear();
  parentMap.clear();
  nextSyntheticNodeId = -1;
}

// Lookup O(1) del genitore di un nodo (usato da TreePanel per ArrowLeft)
export function getParentId(nodeId: number): number | undefined {
  return parentMap.get(nodeId);
}

function registerChildren(parentId: number, children: NodeDto[]) {
  for (const c of children) {
    if (!c.synthetic_kind) parentMap.set(c.id, parentId);
  }
}

function isLoadMoreNode(node: NodeDto): boolean {
  return node.synthetic_kind === "load-more";
}

function makeLoadMoreNode(
  parentId: number,
  nextOffset: number,
  totalCount: number
): NodeDto {
  const remaining = Math.max(totalCount - nextOffset, 0);
  const batch = Math.min(LARGE_NODE_PAGE_SIZE, remaining);
  return {
    id: nextSyntheticNodeId--,
    key: null,
    value_type: "load-more",
    value_preview: `Load ${batch.toLocaleString()} more items (${remaining.toLocaleString()} remaining)`,
    children_count: 0,
    synthetic_kind: "load-more",
    parent_node_id: parentId,
    next_offset: nextOffset,
    remaining_count: remaining
  };
}

function getRealChildren(children: NodeDto[]): NodeDto[] {
  return children.filter((child) => !child.synthetic_kind);
}

function decoratePagedChildren(parentNode: NodeDto, children: NodeDto[], offset: number): NodeDto[] {
  const nextOffset = offset + children.length;
  if (nextOffset >= parentNode.children_count) {
    return children;
  }
  return [...children, makeLoadMoreNode(parentNode.id, nextOffset, parentNode.children_count)];
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

function getKnownNode(nodeId: number, rootChildren: NodeDto[]): NodeDto | null {
  return nodeMapCache.get(nodeId) ?? rootChildren.find((node) => node.id === nodeId) ?? null;
}

function maybeDecoratePartialChildren(
  parentNode: NodeDto | null,
  children: NodeDto[]
): NodeDto[] {
  if (!parentNode || parentNode.children_count <= children.length) {
    return children;
  }
  return decoratePagedChildren(parentNode, children, 0);
}

async function loadChildren(nodeId: number): Promise<NodeDto[]> {
  const cached = childrenCache.get(nodeId);
  if (cached) return cached;

  let inFlight = childrenLoadInFlight.get(nodeId);
  if (!inFlight) {
    inFlight = invoke<NodeDto[]>("get_children", { nodeId }).then((children) => {
      cacheSet(nodeId, children);
      return children;
    });
    childrenLoadInFlight.set(nodeId, inFlight);
    void inFlight.finally(() => {
      if (childrenLoadInFlight.get(nodeId) === inFlight) {
        childrenLoadInFlight.delete(nodeId);
      }
    });
  }
  return inFlight;
}

async function loadChildrenPage(
  nodeId: number,
  offset: number,
  limit: number
): Promise<NodeDto[]> {
  const key = `${nodeId}:${offset}:${limit}`;
  let inFlight = childrenPageLoadInFlight.get(key);
  if (!inFlight) {
    inFlight = invoke<NodeDto[]>("get_children_page", { nodeId, offset, limit });
    childrenPageLoadInFlight.set(key, inFlight);
    void inFlight.finally(() => {
      if (childrenPageLoadInFlight.get(key) === inFlight) {
        childrenPageLoadInFlight.delete(key);
      }
    });
  }
  return inFlight;
}

function loadRecentFiles(): string[] {
  try {
    return JSON.parse(localStorage.getItem("recentFiles") ?? "[]");
  } catch {
    return [];
  }
}

async function fetchRuntimeStats(): Promise<RuntimeStats | null> {
  try {
    const stats = await invoke<Omit<RuntimeStats, "sampled_at_ms">>("get_runtime_stats");
    return {
      ...stats,
      sampled_at_ms: Date.now()
    };
  } catch (err) {
    console.error("runtime stats error:", err);
    return null;
  }
}

async function finishOperationMeasurement(
  label: string,
  startedAtMs: number,
  set: (partial: Partial<JsonStore>) => void,
  get: () => JsonStore
) {
  const stats = await fetchRuntimeStats();
  const runtimeStats = stats ?? get().runtimeStats;
  const update: Partial<JsonStore> = {
    lastOperation: {
      label,
      duration_ms: performance.now() - startedAtMs,
      cpu_percent: runtimeStats?.cpu_percent ?? null,
      resident_bytes: runtimeStats?.resident_bytes ?? null,
      finished_at_ms: Date.now()
    }
  };
  if (stats) {
    update.runtimeStats = stats;
  }
  set(update);
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

export function buildVisibleSubtreeSizeMap(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>
): Map<number, number> {
  const sizeMap = new Map<number, number>();
  const stack: Array<{ node: NodeDto; visited: boolean }> = [];

  for (let i = rootChildren.length - 1; i >= 0; i -= 1) {
    stack.push({ node: rootChildren[i], visited: false });
  }

  while (stack.length > 0) {
    const frame = stack.pop()!;
    const children = expandedNodes.get(frame.node.id);

    if (frame.visited) {
      let size = 1;
      if (children && children.length > 0) {
        for (const child of children) {
          size += sizeMap.get(child.id) ?? 1;
        }
      }
      sizeMap.set(frame.node.id, size);
      continue;
    }

    stack.push({ node: frame.node, visited: true });
    if (children && children.length > 0) {
      for (let i = children.length - 1; i >= 0; i -= 1) {
        stack.push({ node: children[i], visited: false });
      }
    }
  }

  return sizeMap;
}

export function countVisibleNodes(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>,
  sizeMap: Map<number, number> = buildVisibleSubtreeSizeMap(rootChildren, expandedNodes)
): number {
  let total = 0;
  for (const node of rootChildren) {
    total += sizeMap.get(node.id) ?? 1;
  }
  return total;
}

export function getVisibleSlice(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>,
  offset: number,
  limit: number,
  sizeMap: Map<number, number> = buildVisibleSubtreeSizeMap(rootChildren, expandedNodes)
): VNode[] {
  if (limit <= 0) return [];

  const rows: VNode[] = [];
  const stack: Array<{ nodes: NodeDto[]; depth: number; index: number }> = [
    { nodes: rootChildren, depth: 0, index: 0 }
  ];
  let skipped = 0;

  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame.index >= frame.nodes.length) {
      stack.pop();
      continue;
    }

    const node = frame.nodes[frame.index++];
    const span = sizeMap.get(node.id) ?? 1;
    const children = expandedNodes.get(node.id);

    if (skipped < offset) {
      if (skipped + span <= offset) {
        skipped += span;
        continue;
      }
      skipped += 1;
      if (children && children.length > 0) {
        stack.push({ nodes: children, depth: frame.depth + 1, index: 0 });
      }
      continue;
    }

    rows.push({ node, depth: frame.depth });
    if (rows.length >= limit) {
      break;
    }

    if (children && children.length > 0) {
      stack.push({ nodes: children, depth: frame.depth + 1, index: 0 });
    }
  }

  return rows;
}

export function findVisibleNodeIndex(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>,
  nodeId: number
): number {
  const stack: Array<NodeDto[]> = [rootChildren];
  const indexStack: number[] = [0];
  let visibleIndex = 0;

  while (stack.length > 0) {
    const nodes = stack[stack.length - 1];
    const nodeIndex = indexStack[indexStack.length - 1];

    if (nodeIndex >= nodes.length) {
      stack.pop();
      indexStack.pop();
      continue;
    }

    const node = nodes[nodeIndex];
    indexStack[indexStack.length - 1] += 1;

    if (node.id === nodeId) {
      return visibleIndex;
    }
    visibleIndex += 1;

    const children = expandedNodes.get(node.id);
    if (children && children.length > 0) {
      stack.push(children);
      indexStack.push(0);
    }
  }

  return -1;
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
  runtimeStats: RuntimeStats | null;
  lastOperation: OperationMetrics | null;
  contextMenu: ContextMenuState | null;
  openFile: (path: string) => Promise<void>;
  openFromString: (content: string) => Promise<void>;
  toggleNode: (nodeId: number) => Promise<void>;
  loadMoreChildren: (parentId: number, offset: number) => Promise<void>;
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
  refreshRuntimeStats: () => Promise<void>;
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
  runtimeStats: null,
  lastOperation: null,
  contextMenu: null,

  openFile: async (path: string) => {
    const startedAtMs = performance.now();
    set({ loading: true });
    try {
      const info = await invoke<FileInfo>("open_file", { path });
      const expandedNodes = new Map<number, NodeDto[]>();

      resetTreeCaches();

      // Cache root node so loadMoreChildren can look it up by id.
      nodeMapCache.set(info.root_node.id, info.root_node);
      for (const n of info.root_children) {
        nodeMapCache.set(n.id, n);
      }

      // When the root has more children than what the backend returned (paged),
      // seed the children cache and add a load-more sentinel exactly as done
      // for expanded non-root nodes with many children.
      const rootChildren =
        info.root_node.children_count > LARGE_NODE_PAGE_SIZE
          ? (() => {
              cacheSet(info.root_node.id, info.root_children);
              return decoratePagedChildren(info.root_node, info.root_children, 0);
            })()
          : info.root_children;

      const prev = get().recentFiles.filter((f) => f !== path);
      const recentFiles = [path, ...prev].slice(0, MAX_RECENT);
      localStorage.setItem("recentFiles", JSON.stringify(recentFiles));

      set({
        filePath: path,
        nodeCount: info.node_count,
        sizeBytes: info.size_bytes,
        rootNode: info.root_node,
        rootChildren,
        expandedNodes,
        selectedNodeId: null,
        selectedNode: null,
        selectedNodePath: null,
        selectedNodeSiblings: null,
        focusedNodeId: null,
        recentFiles,
        activeSearchMode: null,
        hasActiveSearch: false,
        searchScopePath: "",
        lastSearchQuery: "",
        searchResults: []
      });
    } finally {
      set({ loading: false });
      await finishOperationMeasurement("file-load", startedAtMs, set, get);
    }
  },

  openFromString: async (content: string) => {
    const startedAtMs = performance.now();
    set({ loading: true });
    try {
      const info = await invoke<FileInfo>("open_from_string", {
        content
      });
      const expandedNodes = new Map<number, NodeDto[]>();

      resetTreeCaches();

      nodeMapCache.set(info.root_node.id, info.root_node);
      for (const n of info.root_children) {
        nodeMapCache.set(n.id, n);
      }

      const rootChildren =
        info.root_node.children_count > LARGE_NODE_PAGE_SIZE
          ? (() => {
              cacheSet(info.root_node.id, info.root_children);
              return decoratePagedChildren(info.root_node, info.root_children, 0);
            })()
          : info.root_children;

      set({
        filePath: "(incollato)",
        nodeCount: info.node_count,
        sizeBytes: info.size_bytes,
        rootNode: info.root_node,
        rootChildren,
        expandedNodes,
        selectedNodeId: null,
        selectedNode: null,
        selectedNodePath: null,
        selectedNodeSiblings: null,
        focusedNodeId: null,
        activeSearchMode: null,
        hasActiveSearch: false,
        searchScopePath: "",
        lastSearchQuery: "",
        searchResults: []
      });
    } finally {
      set({ loading: false });
      await finishOperationMeasurement("string-load", startedAtMs, set, get);
    }
  },

  toggleNode: async (nodeId: number) => {
    const startedAtMs = performance.now();
    const { rootChildren } = get();
    const targetNode = getKnownNode(nodeId, rootChildren);
    if (!targetNode || isLoadMoreNode(targetNode) || targetNode.children_count === 0) {
      return;
    }
    const expandedNow = desiredExpandedState.get(nodeId) ?? get().expandedNodes.has(nodeId);
    const shouldExpand = !expandedNow;

    if (!shouldExpand) {
      if (childrenLoadInFlight.has(nodeId)) {
        desiredExpandedState.set(nodeId, false);
      } else {
        desiredExpandedState.delete(nodeId);
      }
      set((state) => {
        if (!state.expandedNodes.has(nodeId)) return state;
        const next = new Map(state.expandedNodes);
        next.delete(nodeId);
        const selectedNodeSiblings =
          state.selectedNodeId !== null
            ? findSiblings(state.selectedNodeId, state.rootChildren, next)
            : null;
        return {
          expandedNodes: next,
          selectedNodeSiblings
        };
      });
      await finishOperationMeasurement("node-collapse", startedAtMs, set, get);
      return;
    }

    desiredExpandedState.set(nodeId, true);
    const cachedChildren = childrenCache.get(nodeId);
    let children = cachedChildren;
    if (!children) {
      if (targetNode.children_count > LARGE_NODE_PAGE_SIZE) {
        const firstPage = await loadChildrenPage(nodeId, 0, LARGE_NODE_PAGE_SIZE);
        if (desiredExpandedState.get(nodeId) !== true) {
          return;
        }
        children = decoratePagedChildren(targetNode, firstPage, 0);
        cacheSet(nodeId, children);
      } else {
        children = await loadChildren(nodeId);
        if (desiredExpandedState.get(nodeId) !== true) {
          return;
        }
      }
    }
    if (desiredExpandedState.get(nodeId) !== true) {
      return;
    }

    const realChildren = getRealChildren(children);
    for (const child of realChildren) {
      nodeMapCache.set(child.id, child);
    }
    registerChildren(nodeId, realChildren);

    set((state) => {
      if (desiredExpandedState.get(nodeId) !== true || state.expandedNodes.has(nodeId)) {
        return state;
      }
      const next = new Map(state.expandedNodes);
      next.set(nodeId, children);
      const selectedNodeSiblings =
        state.selectedNodeId !== null
          ? findSiblings(state.selectedNodeId, state.rootChildren, next)
          : null;
      return {
        expandedNodes: next,
        selectedNodeSiblings
      };
    });

    if (desiredExpandedState.get(nodeId) === true) {
      desiredExpandedState.delete(nodeId);
    }
    await finishOperationMeasurement("node-expansion", startedAtMs, set, get);
  },

  loadMoreChildren: async (parentId: number, offset: number) => {
    const startedAtMs = performance.now();
    const { rootChildren, rootNode } = get();
    const isRoot = rootNode !== null && parentId === rootNode.id;
    const parentNode = isRoot ? rootNode : getKnownNode(parentId, rootChildren);
    if (!parentNode || parentNode.children_count <= LARGE_NODE_PAGE_SIZE) {
      return;
    }

    const page = await loadChildrenPage(parentId, offset, LARGE_NODE_PAGE_SIZE);
    const existing = childrenCache.get(parentId) ?? [];
    const existingRealChildren = getRealChildren(existing);
    if (existingRealChildren.length > offset) {
      return;
    }
    const mergedRealChildren = [...existingRealChildren, ...page];
    const nextChildren = decoratePagedChildren(parentNode, mergedRealChildren, 0);
    cacheSet(parentId, nextChildren);

    for (const child of page) {
      nodeMapCache.set(child.id, child);
    }
    registerChildren(parentId, page);

    if (isRoot) {
      // Root children live in rootChildren state, not in expandedNodes.
      set((state) => ({
        rootChildren: nextChildren,
        selectedNodeSiblings:
          state.selectedNodeId !== null
            ? findSiblings(state.selectedNodeId, nextChildren, state.expandedNodes)
            : null
      }));
    } else {
      set((state) => {
        if (!state.expandedNodes.has(parentId)) {
          return state;
        }
        const next = new Map(state.expandedNodes);
        next.set(parentId, nextChildren);
        const selectedNodeSiblings =
          state.selectedNodeId !== null
            ? findSiblings(state.selectedNodeId, state.rootChildren, next)
            : null;
        return {
          expandedNodes: next,
          selectedNodeSiblings
        };
      });
    }
    await finishOperationMeasurement("node-expansion-page", startedAtMs, set, get);
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
    const startedAtMs = performance.now();
    try {
      const result = await invoke<ExpandToResult>("expand_to", { nodeId });
      const { expandedNodes, rootChildren } = get();
      desiredExpandedState.clear();
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

      const targetNode = nodeMapCache.get(nodeId) ?? null;
      const selectedNodeSiblings = findSiblings(nodeId, rootChildren, next);
      set({
        expandedNodes: next,
        selectedNodeId: nodeId,
        selectedNode: targetNode,
        selectedNodePath: result.path,
        selectedNodeSiblings,
        focusedNodeId: nodeId
      });
    } finally {
      await finishOperationMeasurement("node-navigation", startedAtMs, set, get);
    }
  },

  expandAll: async () => {
    const { rootNode, rootChildren } = get();
    if (rootChildren.length === 0 || !rootNode) return;
    const startedAtMs = performance.now();
    set({ loading: true });
    try {
      const expansions = await invoke<[number, NodeDto[]][]>("expand_subtree", {
        nodeId: rootNode.id,
        maxNodes: MAX_SAFE_EXPAND_NODES
      });
      const next = new Map<number, NodeDto[]>();
      for (const [parentId, children] of expansions) {
        const parentNode =
          parentId === rootNode.id ? rootNode : getKnownNode(parentId, rootChildren);
        const nextChildren = maybeDecoratePartialChildren(parentNode, children);
        next.set(parentId, nextChildren);
        cacheSet(parentId, nextChildren);
        registerChildren(parentId, children);
        for (const child of children) nodeMapCache.set(child.id, child);
      }
      desiredExpandedState.clear();
      set({ expandedNodes: next });
    } catch (err) {
      console.error("expandAll failed:", err);
    } finally {
      set({ loading: false });
      await finishOperationMeasurement("expand-all-tree", startedAtMs, set, get);
    }
  },

  expandSubtree: async (nodeId: number) => {
    const startedAtMs = performance.now();
    try {
      const { expandedNodes, rootChildren } = get();
      const expansions = await invoke<[number, NodeDto[]][]>("expand_subtree", {
        nodeId,
        maxNodes: MAX_SAFE_EXPAND_NODES
      });
      const next = new Map(expandedNodes);
      for (const [parentId, children] of expansions) {
        const parentNode = getKnownNode(parentId, rootChildren);
        const nextChildren = maybeDecoratePartialChildren(parentNode, children);
        next.set(parentId, nextChildren);
        cacheSet(parentId, nextChildren);
        registerChildren(parentId, children);
        for (const child of children) {
          nodeMapCache.set(child.id, child);
        }
      }
      desiredExpandedState.clear();
      set({ expandedNodes: next });
    } finally {
      await finishOperationMeasurement("expand-subtree", startedAtMs, set, get);
    }
  },

  collapseAll: () => {
    const { rootChildren, selectedNodeId } = get();
    const next = new Map<number, NodeDto[]>();
    desiredExpandedState.clear();
    const selectedNodeSiblings =
      selectedNodeId !== null
        ? findSiblings(selectedNodeId, rootChildren, next)
        : null;
    set({ expandedNodes: next, selectedNodeSiblings });
  },

  clearSearch: () =>
    set({
      searchResults: [],
      searching: false,
      lastSearchQuery: "",
      activeSearchMode: null,
      hasActiveSearch: false
    }),

  refreshRuntimeStats: async () => {
    const stats = await fetchRuntimeStats();
    if (stats) {
      set({ runtimeStats: stats });
    }
  },

  showContextMenu: (cm: ContextMenuState) => set({ contextMenu: cm }),
  hideContextMenu: () => set({ contextMenu: null })
}));

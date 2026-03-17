import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface NodeDto {
  id: number;
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
  path: string;
  key: string | null;
  value_preview: string;
}

interface FileInfo {
  node_count: number;
  size_bytes: number;
  root_children: NodeDto[];
}

interface ExpandToResult {
  expansions: [number, NodeDto[]][];
  path: string;
}

const MAX_RECENT = 5;
// Contatore generazione per ignorare chunk di expand_all superati da un reload
let expandGeneration = 0;
const MAX_CHILDREN_CACHE = 2000;

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

export interface ContextMenuState {
  x: number;
  y: number;
  nodeId: number;
  valueType: string;
  valuePreview: string;
}

interface JsonStore {
  filePath: string | null;
  nodeCount: number;
  sizeBytes: number;
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
    exactMatch: boolean
  ) => Promise<void>;
  expandAll: () => Promise<void>;
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
  expandProgress: null,
  selectedNodeId: null,
  selectedNode: null,
  selectedNodePath: null,
  focusedNodeId: null,
  visibleNodes: [],
  selectedNodeSiblings: null,
  recentFiles: loadRecentFiles(),
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
      rootChildren: info.root_children,
      expandedNodes,
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
      recentFiles,
      searchResults: []
    });
  },

  openFromString: async (content: string) => {
    set({ loading: true });
    const info = await invoke<FileInfo>("open_from_string", { content }).finally(() =>
      set({ loading: false })
    );
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
      rootChildren: info.root_children,
      expandedNodes,
      selectedNodeId: null,
      selectedNode: null,
      selectedNodePath: null,
      selectedNodeSiblings: null,
      focusedNodeId: null,
      visibleNodes,
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
    const selectedNodeSiblings = selectedNodeId !== null
      ? findSiblings(selectedNodeId, rootChildren, next)
      : null;
    set({ expandedNodes: next, visibleNodes, selectedNodeSiblings });
  },

  selectNode: async (node: NodeDto) => {
    let path = pathCache.get(node.id);
    if (!path) {
      path = await invoke<string>("get_path", { nodeId: node.id });
      pathCache.set(node.id, path);
    }
    const { rootChildren, expandedNodes } = get();
    const selectedNodeSiblings = findSiblings(node.id, rootChildren, expandedNodes);
    set({
      selectedNodeId: node.id,
      selectedNode: node,
      selectedNodePath: path,
      selectedNodeSiblings,
      focusedNodeId: node.id
    });
  },

  setFocusedNode: (nodeId: number | null) => {
    set({ focusedNodeId: nodeId });
  },

  search: async (
    query: string,
    target: string,
    caseSensitive: boolean,
    useRegex: boolean,
    exactMatch: boolean
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
          max_results: 500
        }
      });
      set({ searchResults: results, searching: false });
    } catch (err) {
      console.error("Search error:", err);
      set({ searching: false });
    }
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
    const { rootChildren } = get();
    if (rootChildren.length === 0) return;

    expandGeneration++;
    const myGen = expandGeneration;
    set({ loading: true });

    // Accumulatore locale: cresce chunk per chunk
    const next = new Map<number, NodeDto[]>();
    // Chunk ricevuti ma non ancora applicati all'UI
    let pending: [number, NodeDto[]][] = [];
    let throttleTimer: ReturnType<typeof setTimeout> | null = null;

    let doneResolve!: () => void;
    const donePromise = new Promise<void>((r) => { doneResolve = r; });

    // Applica i chunk pendenti e aggiorna l'UI (throttled)
    const applyPending = () => {
      throttleTimer = null;
      if (pending.length === 0) return;
      const toApply = pending.splice(0);
      for (const [id, children] of toApply) {
        next.set(id, children);
        cacheSet(id, children);
        registerChildren(id, children);
        for (const child of children) nodeMapCache.set(child.id, child);
      }
      const { rootChildren: rc, selectedNodeId } = get();
      const visibleNodes = buildVisibleNodes(rc, next);
      const selectedNodeSiblings =
        selectedNodeId !== null ? findSiblings(selectedNodeId, rc, next) : null;
      set({ expandedNodes: new Map(next), visibleNodes, selectedNodeSiblings });
    };

    const unlistenChunk = await listen<{ expansions: [number, NodeDto[]][], progress: number }>(
      "expand-chunk",
      (e) => {
        if (expandGeneration !== myGen) return;
        pending.push(...e.payload.expansions);
        set({ expandProgress: e.payload.progress });
        if (!throttleTimer) throttleTimer = setTimeout(applyPending, 100);
      }
    );

    const unlistenDone = await listen("expand-done", () => {
      if (expandGeneration !== myGen) { doneResolve(); return; }
      if (throttleTimer) { clearTimeout(throttleTimer); throttleTimer = null; }
      // Applica eventuali chunk rimasti
      applyPending();
      unlistenChunk();
      unlistenDone();
      set({ loading: false, expandProgress: null });
      doneResolve();
    });

    try {
      await invoke("expand_all");
    } catch (err) {
      console.error("expand_all failed:", err);
      if (throttleTimer) clearTimeout(throttleTimer);
      unlistenChunk();
      unlistenDone();
      set({ loading: false, expandProgress: null });
      doneResolve();
    }

    await donePromise;
  },

  collapseAll: () => {
    const { rootChildren, selectedNodeId } = get();
    const next = new Map<number, NodeDto[]>();
    const visibleNodes = buildVisibleNodes(rootChildren, next);
    const selectedNodeSiblings = selectedNodeId !== null
      ? findSiblings(selectedNodeId, rootChildren, next)
      : null;
    set({ expandedNodes: next, visibleNodes, selectedNodeSiblings });
  },

  clearSearch: () => set({ searchResults: [], searching: false }),

  showContextMenu: (cm: ContextMenuState) => set({ contextMenu: cm }),
  hideContextMenu: () => set({ contextMenu: null })
}));

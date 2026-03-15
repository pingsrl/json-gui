import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'

export interface NodeDto {
  id: number
  key: string | null
  value_type: string
  value_preview: string
  has_children: boolean
  children_count: number
}

export interface SearchResult {
  node_id: number
  path: string
  key: string | null
  value_preview: string
}

interface FileInfo {
  node_count: number
  size_bytes: number
  root_children: NodeDto[]
}

const MAX_RECENT = 5

function loadRecentFiles(): string[] {
  try {
    return JSON.parse(localStorage.getItem('recentFiles') ?? '[]')
  } catch {
    return []
  }
}

function buildVisibleNodes(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>,
): NodeDto[] {
  const result: NodeDto[] = []
  function traverse(nodes: NodeDto[]) {
    for (const node of nodes) {
      result.push(node)
      if (expandedNodes.has(node.id)) {
        const children = expandedNodes.get(node.id)!
        traverse(children)
      }
    }
  }
  traverse(rootChildren)
  return result
}

interface JsonStore {
  filePath: string | null
  nodeCount: number
  sizeBytes: number
  rootChildren: NodeDto[]
  expandedNodes: Map<number, NodeDto[]>
  selectedNodeId: number | null
  focusedNodeId: number | null
  visibleNodes: NodeDto[]
  recentFiles: string[]
  searchResults: SearchResult[]
  searching: boolean
  openFile: (path: string) => Promise<void>
  toggleNode: (nodeId: number) => Promise<void>
  navigateToNode: (nodeId: number) => Promise<void>
  setFocusedNode: (nodeId: number | null) => void
  search: (query: string, target: string, caseSensitive: boolean) => Promise<void>
  clearSearch: () => void
}

export const useJsonStore = create<JsonStore>((set, get) => ({
  filePath: null,
  nodeCount: 0,
  sizeBytes: 0,
  rootChildren: [],
  expandedNodes: new Map(),
  selectedNodeId: null,
  focusedNodeId: null,
  visibleNodes: [],
  recentFiles: loadRecentFiles(),
  searchResults: [],
  searching: false,

  openFile: async (path: string) => {
    const info = await invoke<FileInfo>('open_file', { path })
    const expandedNodes = new Map<number, NodeDto[]>()
    const visibleNodes = buildVisibleNodes(info.root_children, expandedNodes)

    // Update recent files
    const prev = get().recentFiles.filter((f) => f !== path)
    const recentFiles = [path, ...prev].slice(0, MAX_RECENT)
    localStorage.setItem('recentFiles', JSON.stringify(recentFiles))

    set({
      filePath: path,
      nodeCount: info.node_count,
      sizeBytes: info.size_bytes,
      rootChildren: info.root_children,
      expandedNodes,
      selectedNodeId: null,
      focusedNodeId: null,
      visibleNodes,
      recentFiles,
      searchResults: [],
    })
  },

  toggleNode: async (nodeId: number) => {
    const { expandedNodes, rootChildren } = get()
    let next: Map<number, NodeDto[]>
    if (expandedNodes.has(nodeId)) {
      next = new Map(expandedNodes)
      next.delete(nodeId)
    } else {
      const children = await invoke<NodeDto[]>('get_children', { nodeId })
      next = new Map(expandedNodes)
      next.set(nodeId, children)
    }
    const visibleNodes = buildVisibleNodes(rootChildren, next)
    set({ expandedNodes: next, visibleNodes })
  },

  setFocusedNode: (nodeId: number | null) => {
    set({ focusedNodeId: nodeId })
  },

  search: async (query: string, target: string, caseSensitive: boolean) => {
    if (!query.trim()) {
      get().clearSearch()
      return
    }
    set({ searching: true })
    try {
      const results = await invoke<SearchResult[]>('search', {
        query: {
          text: query,
          target,
          case_sensitive: caseSensitive,
          regex: false,
          max_results: 500,
        },
      })
      set({ searchResults: results, searching: false })
    } catch (err) {
      console.error('Search error:', err)
      set({ searching: false })
    }
  },

  navigateToNode: async (nodeId: number) => {
    const expansions = await invoke<[number, NodeDto[]][]>('expand_to', { nodeId })
    const { expandedNodes, rootChildren } = get()
    const next = new Map(expandedNodes)
    for (const [id, children] of expansions) {
      next.set(id, children)
    }
    const visibleNodes = buildVisibleNodes(rootChildren, next)
    set({ expandedNodes: next, selectedNodeId: nodeId, focusedNodeId: nodeId, visibleNodes })
    setTimeout(() => {
      document.getElementById(`node-${nodeId}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
    }, 50)
  },

  clearSearch: () => set({ searchResults: [], searching: false }),
}))

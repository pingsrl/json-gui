import { useState, useCallback, useEffect, useRef } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { useJsonStore, NodeDto } from './store'
import { TreeNode } from './components/TreeNode'
import { FolderOpen, Search, X, Clock } from 'lucide-react'

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`
}

export default function App() {
  const {
    filePath,
    nodeCount,
    sizeBytes,
    rootChildren,
    expandedNodes,
    searchResults,
    searching,
    focusedNodeId,
    visibleNodes,
    recentFiles,
    openFile,
    navigateToNode,
    search,
    clearSearch,
    toggleNode,
    setFocusedNode,
  } = useJsonStore()

  const [searchQuery, setSearchQuery] = useState('')
  const [searchTarget, setSearchTarget] = useState('both')
  const [caseSensitive, setCaseSensitive] = useState(false)
  const [isDragging, setIsDragging] = useState(false)
  const [recentOpen, setRecentOpen] = useState(false)
  const recentRef = useRef<HTMLDivElement>(null)

  const handleOpenFile = async () => {
    const selected = await open({
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
    if (selected) {
      await openFile(selected as string)
    }
  }

  const handleSearch = useCallback(
    async (q: string) => {
      setSearchQuery(q)
      await search(q, searchTarget, caseSensitive)
    },
    [search, searchTarget, caseSensitive],
  )

  const handleClear = () => {
    setSearchQuery('')
    clearSearch()
  }

  // Keyboard shortcut: Cmd+F / Ctrl+F to focus search
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'f') {
        e.preventDefault()
        document.getElementById('search-input')?.focus()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [])

  // Close recent dropdown when clicking outside
  useEffect(() => {
    if (!recentOpen) return
    const handler = (e: MouseEvent) => {
      if (recentRef.current && !recentRef.current.contains(e.target as Node)) {
        setRecentOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [recentOpen])

  // Keyboard navigation in tree
  useEffect(() => {
    if (rootChildren.length === 0) return
    const handler = (e: KeyboardEvent) => {
      // Don't intercept when typing in inputs
      const tag = (e.target as HTMLElement).tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA') return

      if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Enter'].includes(e.key)) return
      e.preventDefault()

      const currentId = focusedNodeId
      const idx = currentId !== null ? visibleNodes.findIndex((n) => n.id === currentId) : -1

      if (e.key === 'ArrowDown') {
        const next = idx < visibleNodes.length - 1 ? visibleNodes[idx + 1] : visibleNodes[0]
        if (next) {
          setFocusedNode(next.id)
          document.getElementById(`node-${next.id}`)?.scrollIntoView({ block: 'nearest' })
        }
      } else if (e.key === 'ArrowUp') {
        const prev = idx > 0 ? visibleNodes[idx - 1] : visibleNodes[visibleNodes.length - 1]
        if (prev) {
          setFocusedNode(prev.id)
          document.getElementById(`node-${prev.id}`)?.scrollIntoView({ block: 'nearest' })
        }
      } else if (e.key === 'ArrowRight') {
        if (currentId === null) return
        const node = visibleNodes[idx]
        if (!node) return
        if (node.has_children && !expandedNodes.has(node.id)) {
          toggleNode(node.id)
        }
      } else if (e.key === 'ArrowLeft') {
        if (currentId === null) return
        const node = visibleNodes[idx]
        if (!node) return
        if (expandedNodes.has(node.id)) {
          toggleNode(node.id)
        } else {
          // Find parent by scanning all nodes in rootChildren tree
          // Use the DOM structure: go to parent node in visible list
          // We look backwards in visibleNodes for a node at a shallower depth
          // Since we don't store depth in NodeDto, we find the parent via the node's key structure
          // Simpler: find the node that contains currentId in expandedNodes
          for (const [parentId, children] of expandedNodes.entries()) {
            if (children.some((c) => c.id === node.id)) {
              setFocusedNode(parentId)
              document.getElementById(`node-${parentId}`)?.scrollIntoView({ block: 'nearest' })
              break
            }
          }
        }
      } else if (e.key === 'Enter') {
        if (currentId === null) return
        const node = visibleNodes[idx]
        if (node?.has_children) {
          toggleNode(node.id)
        }
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [focusedNodeId, visibleNodes, expandedNodes, rootChildren, toggleNode, setFocusedNode])

  // Drag & drop file dalla Finder/OS
  useEffect(() => {
    let unlistenFn: (() => void) | undefined
    getCurrentWebviewWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'enter' || event.payload.type === 'over') {
          setIsDragging(true)
        } else if (event.payload.type === 'leave') {
          setIsDragging(false)
        } else if (event.payload.type === 'drop') {
          setIsDragging(false)
          const paths: string[] = (event.payload as { type: 'drop'; paths: string[] }).paths
          const jsonFile = paths.find((p) => p.toLowerCase().endsWith('.json'))
          if (jsonFile) openFile(jsonFile)
        }
      })
      .then((unlisten) => {
        unlistenFn = unlisten
      })
    return () => unlistenFn?.()
  }, [openFile])

  return (
    <div className="h-screen flex flex-col bg-gray-900 text-gray-100 relative">
      {/* Overlay drag & drop */}
      {isDragging && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-blue-900/60 border-4 border-dashed border-blue-400 pointer-events-none">
          <div className="text-blue-200 text-lg font-medium">Rilascia il file JSON</div>
        </div>
      )}
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-2 bg-gray-800 border-b border-gray-700">
        <button
          onClick={handleOpenFile}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 hover:bg-blue-500 rounded text-sm font-medium transition-colors"
        >
          <FolderOpen size={14} />
          Apri file
        </button>

        {/* Recent files dropdown */}
        {recentFiles.length > 0 && (
          <div className="relative" ref={recentRef}>
            <button
              onClick={() => setRecentOpen((v) => !v)}
              className="flex items-center gap-1.5 px-2 py-1.5 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors"
              title="File recenti"
            >
              <Clock size={14} />
              Recenti
            </button>
            {recentOpen && (
              <div className="absolute left-0 top-full mt-1 z-40 bg-gray-800 border border-gray-600 rounded shadow-lg py-1 min-w-[280px]">
                {recentFiles.map((rf) => (
                  <button
                    key={rf}
                    className="w-full text-left px-3 py-1.5 text-xs font-mono text-gray-300 hover:bg-gray-700 truncate transition-colors"
                    title={rf}
                    onClick={() => {
                      setRecentOpen(false)
                      openFile(rf)
                    }}
                  >
                    {rf}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        <span className="text-gray-400 text-sm truncate flex-1">
          {filePath ?? 'Nessun file aperto'}
        </span>
      </div>

      <div className="flex flex-1 overflow-hidden">
        {/* Tree panel */}
        <div className="flex-1 overflow-auto border-r border-gray-700">
          {rootChildren.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-gray-500 gap-3">
              <FolderOpen size={40} className="opacity-30" />
              <span className="text-sm">Apri un file JSON per iniziare</span>
              <span className="text-xs opacity-50">Supporta file di qualsiasi dimensione</span>
            </div>
          ) : (
            <div className="py-1">
              {rootChildren.map((node: NodeDto) => (
                <TreeNode key={node.id} node={node} depth={0} />
              ))}
            </div>
          )}
        </div>

        {/* Search panel */}
        <div className="w-80 flex flex-col bg-gray-900">
          <div className="p-3 border-b border-gray-700">
            <div className="relative">
              <Search
                size={14}
                className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500"
              />
              <input
                id="search-input"
                type="text"
                placeholder="Cerca... (Cmd+F)"
                value={searchQuery}
                onChange={(e) => handleSearch(e.target.value)}
                disabled={nodeCount === 0}
                className="w-full pl-8 pr-8 py-1.5 bg-gray-700 border border-gray-600 rounded text-sm placeholder-gray-500 focus:outline-none focus:border-blue-500 disabled:opacity-40 disabled:cursor-not-allowed"
              />
              {searchQuery && (
                <button
                  onClick={handleClear}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300"
                >
                  <X size={12} />
                </button>
              )}
            </div>

            <div className="mt-2 flex gap-3 flex-wrap">
              {(['both', 'keys', 'values'] as const).map((t) => (
                <label
                  key={t}
                  className="flex items-center gap-1 text-xs text-gray-400 cursor-pointer"
                >
                  <input
                    type="radio"
                    name="target"
                    value={t}
                    checked={searchTarget === t}
                    onChange={() => setSearchTarget(t)}
                    className="accent-blue-500"
                  />
                  {t === 'both' ? 'entrambi' : t === 'keys' ? 'chiavi' : 'valori'}
                </label>
              ))}
            </div>

            <label className="mt-1.5 flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer">
              <input
                type="checkbox"
                checked={caseSensitive}
                onChange={(e) => setCaseSensitive(e.target.checked)}
                className="accent-blue-500"
              />
              case sensitive
            </label>
          </div>

          <div className="flex-1 overflow-auto">
            {searching && (
              <div className="p-3 text-gray-500 text-xs">Ricerca in corso...</div>
            )}
            {!searching && searchResults.length > 0 && (
              <div>
                <div className="px-3 py-1.5 text-xs text-gray-500 border-b border-gray-700 sticky top-0 bg-gray-900">
                  {searchResults.length} risultati
                  {searchResults.length === 500 && (
                    <span className="text-yellow-600 ml-1">(limite raggiunto)</span>
                  )}
                </div>
                {searchResults.map((r) => (
                  <div
                    key={r.node_id}
                    onClick={() => navigateToNode(r.node_id)}
                    className="px-3 py-2 hover:bg-gray-700 cursor-pointer border-b border-gray-800"
                  >
                    <div className="text-xs text-blue-400 font-mono truncate">{r.path}</div>
                    <div className="text-xs text-gray-300 font-mono truncate mt-0.5">
                      {r.value_preview}
                    </div>
                  </div>
                ))}
              </div>
            )}
            {!searching && searchQuery && searchResults.length === 0 && (
              <div className="p-3 text-gray-500 text-xs">Nessun risultato trovato</div>
            )}
            {!searching && !searchQuery && nodeCount > 0 && (
              <div className="p-3 text-gray-600 text-xs">
                Digita per cercare tra {nodeCount.toLocaleString()} nodi
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-4 px-3 py-1 bg-gray-800 border-t border-gray-700 text-xs text-gray-500">
        <span>Nodi: {nodeCount.toLocaleString()}</span>
        <span>Dimensione: {formatBytes(sizeBytes)}</span>
        {filePath && (
          <span className="truncate flex-1 text-right" title={filePath}>
            {filePath}
          </span>
        )}
      </div>
    </div>
  )
}

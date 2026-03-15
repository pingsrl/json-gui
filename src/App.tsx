import { useState, useCallback, useEffect, useRef } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { listen } from '@tauri-apps/api/event'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useJsonStore } from './store'
import { TreeNode } from './components/TreeNode'
import { ContextMenu } from './components/ContextMenu'
import { PropertiesPanel } from './components/PropertiesPanel'
import { FolderOpen, Search, X, Clock, Sun, Moon, Download } from 'lucide-react'

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
    loading,
    focusedNodeId,
    visibleNodes,
    recentFiles,
    selectedNodePath,
    selectedNodeId,
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
  const [useRegex, setUseRegex] = useState(false)
  const [isDragging, setIsDragging] = useState(false)
  const [recentOpen, setRecentOpen] = useState(false)
  const [darkMode, setDarkMode] = useState(() => localStorage.getItem('theme') !== 'light')
  const [parseProgress, setParseProgress] = useState<number | null>(null)
  const [updateAvailable, setUpdateAvailable] = useState(false)
  const [updating, setUpdating] = useState(false)
  const recentRef = useRef<HTMLDivElement>(null)
  const treeRef = useRef<HTMLDivElement>(null)
  const searchTimer = useRef<ReturnType<typeof setTimeout>>()

  // Virtualizer per il tree
  const rowVirtualizer = useVirtualizer({
    count: visibleNodes.length,
    getScrollElement: () => treeRef.current,
    estimateSize: () => 24,
    overscan: 20,
  })

  // Dark mode
  useEffect(() => {
    document.documentElement.classList.toggle('dark', darkMode)
    document.documentElement.classList.toggle('light', !darkMode)
    localStorage.setItem('theme', darkMode ? 'dark' : 'light')
  }, [darkMode])

  // Progress events dal backend (file >200MB)
  useEffect(() => {
    let unlisten: (() => void) | undefined
    listen<number>('parse-progress', (e) => setParseProgress(e.payload)).then((fn) => { unlisten = fn })
    return () => unlisten?.()
  }, [])

  useEffect(() => {
    if (!loading) {
      const t = setTimeout(() => setParseProgress(null), 400)
      return () => clearTimeout(t)
    }
  }, [loading])

  // Controlla aggiornamenti all'avvio (silenzioso)
  useEffect(() => {
    check().then((update) => {
      if (update?.available) setUpdateAvailable(true)
    }).catch(() => {})
  }, [])

  // Scroll reattivo: quando cambia il nodo selezionato, scrolla al suo indice
  useEffect(() => {
    if (selectedNodeId === null) return
    const idx = visibleNodes.findIndex(({ node }) => node.id === selectedNodeId)
    if (idx >= 0) {
      rowVirtualizer.scrollToIndex(idx, { align: 'center' })
    }
  }, [selectedNodeId, visibleNodes, rowVirtualizer])

  const handleUpdate = async () => {
    setUpdating(true)
    try {
      const update = await check()
      if (update?.available) {
        await update.downloadAndInstall()
        await relaunch()
      }
    } catch (err) {
      console.error('Update failed:', err)
      setUpdating(false)
    }
  }

  const handleOpenFile = async () => {
    const selected = await open({ filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (selected) await openFile(selected as string)
  }

  const handleSearch = useCallback(
    (q: string) => {
      setSearchQuery(q)
      clearTimeout(searchTimer.current)
      searchTimer.current = setTimeout(() => {
        search(q, searchTarget, caseSensitive, useRegex)
      }, 150)
    },
    [search, searchTarget, caseSensitive, useRegex],
  )

  const handleClear = () => {
    setSearchQuery('')
    clearSearch()
  }

  // Cmd+F
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

  // Chiudi dropdown recenti cliccando fuori
  useEffect(() => {
    if (!recentOpen) return
    const handler = (e: MouseEvent) => {
      if (recentRef.current && !recentRef.current.contains(e.target as Node)) setRecentOpen(false)
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [recentOpen])

  // Navigazione tastiera nel tree
  useEffect(() => {
    if (rootChildren.length === 0) return
    const handler = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement).tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA') return
      if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Enter'].includes(e.key)) return
      e.preventDefault()

      const idx = focusedNodeId !== null
        ? visibleNodes.findIndex(({ node }) => node.id === focusedNodeId)
        : -1

      if (e.key === 'ArrowDown') {
        const nextVNode = idx < visibleNodes.length - 1 ? visibleNodes[idx + 1] : visibleNodes[0]
        if (nextVNode) {
          setFocusedNode(nextVNode.node.id)
          rowVirtualizer.scrollToIndex(idx < visibleNodes.length - 1 ? idx + 1 : 0, { align: 'nearest' })
        }
      } else if (e.key === 'ArrowUp') {
        const prevIdx = idx > 0 ? idx - 1 : visibleNodes.length - 1
        const prevVNode = visibleNodes[prevIdx]
        if (prevVNode) {
          setFocusedNode(prevVNode.node.id)
          rowVirtualizer.scrollToIndex(prevIdx, { align: 'nearest' })
        }
      } else if (e.key === 'ArrowRight') {
        const vNode = visibleNodes[idx]
        if (vNode?.node.has_children && !expandedNodes.has(vNode.node.id)) toggleNode(vNode.node.id)
      } else if (e.key === 'ArrowLeft') {
        const vNode = visibleNodes[idx]
        if (!vNode) return
        if (expandedNodes.has(vNode.node.id)) {
          toggleNode(vNode.node.id)
        } else {
          for (const [parentId, children] of expandedNodes.entries()) {
            if (children.some((c) => c.id === vNode.node.id)) {
              setFocusedNode(parentId)
              const parentIdx = visibleNodes.findIndex(({ node }) => node.id === parentId)
              if (parentIdx >= 0) rowVirtualizer.scrollToIndex(parentIdx, { align: 'nearest' })
              break
            }
          }
        }
      } else if (e.key === 'Enter') {
        const vNode = visibleNodes[idx]
        if (vNode?.node.has_children) toggleNode(vNode.node.id)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [focusedNodeId, visibleNodes, expandedNodes, rootChildren, toggleNode, setFocusedNode, rowVirtualizer])

  // Drag & drop
  useEffect(() => {
    let unlisten: (() => void) | undefined
    getCurrentWebviewWindow().onDragDropEvent((event) => {
      if (event.payload.type === 'enter' || event.payload.type === 'over') setIsDragging(true)
      else if (event.payload.type === 'leave') setIsDragging(false)
      else if (event.payload.type === 'drop') {
        setIsDragging(false)
        const paths = (event.payload as { type: 'drop'; paths: string[] }).paths
        const jsonFile = paths.find((p) => p.toLowerCase().endsWith('.json'))
        if (jsonFile) openFile(jsonFile)
      }
    }).then((fn) => { unlisten = fn })
    return () => unlisten?.()
  }, [openFile])

  return (
    <div className="h-screen flex flex-col bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100 relative">
      {/* Progress bar */}
      {loading && (
        <div className="absolute inset-x-0 top-0 z-50 h-0.5 bg-gray-200 dark:bg-gray-700">
          {parseProgress !== null
            ? <div className="h-full bg-blue-500 transition-all duration-150" style={{ width: `${parseProgress}%` }} />
            : <div className="h-full w-full bg-blue-500 animate-pulse" />}
        </div>
      )}

      {/* Overlay drag & drop */}
      {isDragging && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-blue-100/60 dark:bg-blue-900/60 border-4 border-dashed border-blue-500 dark:border-blue-400 pointer-events-none">
          <div className="text-blue-800 dark:text-blue-200 text-lg font-medium">Rilascia il file JSON</div>
        </div>
      )}

      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-2 bg-white dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <button
          onClick={handleOpenFile}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 hover:bg-blue-500 rounded text-sm font-medium text-white transition-colors"
        >
          <FolderOpen size={14} />
          Apri file
        </button>

        {recentFiles.length > 0 && (
          <div className="relative" ref={recentRef}>
            <button
              onClick={() => setRecentOpen((v) => !v)}
              className="flex items-center gap-1.5 px-2 py-1.5 bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 rounded text-sm transition-colors"
              title="File recenti"
            >
              <Clock size={14} />
              Recenti
            </button>
            {recentOpen && (
              <div className="absolute left-0 top-full mt-1 z-40 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded shadow-lg py-1 min-w-[280px]">
                {recentFiles.map((rf) => (
                  <button
                    key={rf}
                    className="w-full text-left px-3 py-1.5 text-xs font-mono text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 truncate transition-colors"
                    title={rf}
                    onClick={() => { setRecentOpen(false); openFile(rf) }}
                  >
                    {rf}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        <span className="text-gray-400 dark:text-gray-500 text-sm truncate flex-1">
          {filePath ?? 'Nessun file aperto'}
        </span>

        {/* Notifica aggiornamento */}
        {updateAvailable && (
          <button
            onClick={handleUpdate}
            disabled={updating}
            className="flex items-center gap-1.5 px-2 py-1.5 bg-emerald-600 hover:bg-emerald-500 disabled:opacity-60 rounded text-sm text-white transition-colors"
            title="Aggiornamento disponibile"
          >
            <Download size={14} />
            {updating ? 'Aggiornamento...' : 'Aggiorna'}
          </button>
        )}

        <button
          onClick={() => setDarkMode((v) => !v)}
          className="p-1.5 rounded text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
          title={darkMode ? 'Tema chiaro' : 'Tema scuro'}
        >
          {darkMode ? <Sun size={16} /> : <Moon size={16} />}
        </button>
      </div>

      {/* Contenuto principale — 3 colonne */}
      <div className="flex flex-1 overflow-hidden">

        {/* Colonna sinistra: Search */}
        <div className="w-72 flex flex-col border-r border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 flex-shrink-0">
          <div className="p-3 border-b border-gray-200 dark:border-gray-700">
            <div className="relative">
              <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400 dark:text-gray-500" />
              <input
                id="search-input"
                type="text"
                placeholder="Cerca... (Cmd+F)"
                value={searchQuery}
                onChange={(e) => handleSearch(e.target.value)}
                disabled={nodeCount === 0}
                className="w-full pl-8 pr-8 py-1.5 bg-white dark:bg-gray-700 border border-gray-300 dark:border-gray-600 rounded text-sm placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-blue-500 disabled:opacity-40 disabled:cursor-not-allowed text-gray-900 dark:text-gray-100"
              />
              {searchQuery && (
                <button onClick={handleClear} className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300">
                  <X size={12} />
                </button>
              )}
            </div>

            <div className="mt-2 flex gap-3 flex-wrap">
              {(['both', 'keys', 'values'] as const).map((t) => (
                <label key={t} className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
                  <input type="radio" name="target" value={t} checked={searchTarget === t} onChange={() => setSearchTarget(t)} className="accent-blue-500" />
                  {t === 'both' ? 'entrambi' : t === 'keys' ? 'chiavi' : 'valori'}
                </label>
              ))}
            </div>

            <div className="mt-1.5 flex gap-4">
              <label className="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
                <input type="checkbox" checked={caseSensitive} onChange={(e) => setCaseSensitive(e.target.checked)} className="accent-blue-500" />
                case sensitive
              </label>
              <label className="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
                <input type="checkbox" checked={useRegex} onChange={(e) => setUseRegex(e.target.checked)} className="accent-blue-500" />
                regex
              </label>
            </div>
          </div>

          <div className="flex-1 overflow-auto">
            {searching && <div className="p-3 text-gray-400 dark:text-gray-500 text-xs">Ricerca in corso...</div>}
            {!searching && searchResults.length > 0 && (
              <div>
                <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-200 dark:border-gray-700 sticky top-0 bg-gray-50 dark:bg-gray-900">
                  {searchResults.length} risultati
                  {searchResults.length === 500 && <span className="text-yellow-600 ml-1">(limite raggiunto)</span>}
                </div>
                {searchResults.map((r) => (
                  <div
                    key={r.node_id}
                    onClick={() => navigateToNode(r.node_id)}
                    className="px-3 py-2 hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer border-b border-gray-100 dark:border-gray-800"
                  >
                    <div className="text-xs text-blue-600 dark:text-blue-400 font-mono truncate">{r.path}</div>
                    <div className="text-xs text-gray-700 dark:text-gray-300 font-mono truncate mt-0.5">{r.value_preview}</div>
                  </div>
                ))}
              </div>
            )}
            {!searching && searchQuery && searchResults.length === 0 && (
              <div className="p-3 text-gray-400 dark:text-gray-500 text-xs">Nessun risultato trovato</div>
            )}
            {!searching && !searchQuery && nodeCount > 0 && (
              <div className="p-3 text-gray-400 dark:text-gray-600 text-xs">
                Digita per cercare tra {nodeCount.toLocaleString()} nodi
              </div>
            )}
          </div>
        </div>

        {/* Colonna centrale: Tree (virtualizzato) */}
        <div ref={treeRef} className="flex-1 overflow-auto border-r border-gray-200 dark:border-gray-700">
          {rootChildren.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-gray-400 dark:text-gray-500 gap-3">
              <FolderOpen size={40} className="opacity-30" />
              <span className="text-sm">Apri un file JSON per iniziare</span>
              <span className="text-xs opacity-50">Supporta file di qualsiasi dimensione</span>
            </div>
          ) : (
            <div
              style={{ height: `${rowVirtualizer.getTotalSize()}px`, position: 'relative' }}
            >
              {rowVirtualizer.getVirtualItems().map((vItem) => {
                const vNode = visibleNodes[vItem.index]
                return (
                  <div
                    key={vItem.key}
                    style={{
                      position: 'absolute',
                      top: vItem.start,
                      height: 24,
                      width: '100%',
                    }}
                  >
                    <TreeNode node={vNode.node} depth={vNode.depth} />
                  </div>
                )
              })}
            </div>
          )}
        </div>

        {/* Colonna destra: Properties */}
        <div className="w-72 flex flex-col border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex-shrink-0">
          <div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 text-xs font-medium text-gray-500 dark:text-gray-400 flex-shrink-0">
            Proprieta
          </div>
          <div className="flex-1 overflow-hidden">
            <PropertiesPanel />
          </div>
        </div>
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-4 px-3 py-1 bg-white dark:bg-gray-800 border-t border-gray-200 dark:border-gray-700 text-xs text-gray-400 dark:text-gray-500 flex-shrink-0">
        <span>Nodi: {nodeCount.toLocaleString()}</span>
        <span>Dimensione: {formatBytes(sizeBytes)}</span>
        {selectedNodePath && (
          <span className="font-mono text-blue-600 dark:text-blue-400 truncate flex-1" title={selectedNodePath}>
            {selectedNodePath}
          </span>
        )}
        {!selectedNodePath && filePath && (
          <span className="truncate flex-1 text-right" title={filePath}>{filePath}</span>
        )}
      </div>

      {/* Context menu centralizzato */}
      <ContextMenu />
    </div>
  )
}

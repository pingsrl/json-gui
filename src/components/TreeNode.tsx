import { type FC, useState, useEffect, useRef, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { NodeDto, useJsonStore } from '../store'
import { ChevronRight, ChevronDown } from 'lucide-react'

const TYPE_COLORS: Record<string, string> = {
  string: 'text-green-600 dark:text-green-400',
  number: 'text-blue-600 dark:text-blue-400',
  boolean: 'text-amber-600 dark:text-yellow-400',
  null: 'text-gray-400 dark:text-gray-500',
  object: 'text-purple-600 dark:text-purple-400',
  array: 'text-orange-600 dark:text-orange-400',
}

interface Props {
  node: NodeDto
  depth: number
}

interface ContextMenuState {
  x: number
  y: number
  nodeId: number
  valueType: string
  valuePreview: string
}

export const TreeNode: FC<Props> = ({ node, depth }) => {
  const { expandedNodes, toggleNode, selectedNodeId, focusedNodeId, setFocusedNode } = useJsonStore()
  const children = expandedNodes.get(node.id)
  const isExpanded = children !== undefined
  const isSelected = selectedNodeId === node.id
  const isFocused = focusedNodeId === node.id

  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  const handleClick = () => {
    if (node.has_children) {
      toggleNode(node.id)
    }
    setFocusedNode(node.id)
  }

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault()
    setFocusedNode(node.id)
    setContextMenu({
      x: e.clientX,
      y: e.clientY,
      nodeId: node.id,
      valueType: node.value_type,
      valuePreview: node.value_preview,
    })
  }

  useEffect(() => {
    if (!contextMenu) return
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setContextMenu(null)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [contextMenu])

  const copyPath = useCallback(async () => {
    try {
      const path = await invoke<string>('get_path', { nodeId: contextMenu!.nodeId })
      await navigator.clipboard.writeText(path)
    } catch (err) {
      console.error('copyPath error:', err)
    }
    setContextMenu(null)
  }, [contextMenu])

  const copyValue = useCallback(async () => {
    try {
      const vt = contextMenu!.valueType
      if (vt === 'object' || vt === 'array') {
        const raw = await invoke<string>('get_raw', { nodeId: contextMenu!.nodeId })
        await navigator.clipboard.writeText(raw)
      } else {
        await navigator.clipboard.writeText(contextMenu!.valuePreview)
      }
    } catch (err) {
      console.error('copyValue error:', err)
    }
    setContextMenu(null)
  }, [contextMenu])

  const copyRaw = useCallback(async () => {
    try {
      const raw = await invoke<string>('get_raw', { nodeId: contextMenu!.nodeId })
      const pretty = JSON.stringify(JSON.parse(raw), null, 2)
      await navigator.clipboard.writeText(pretty)
    } catch (err) {
      console.error('copyRaw error:', err)
    }
    setContextMenu(null)
  }, [contextMenu])

  return (
    <div>
      <div
        id={`node-${node.id}`}
        className={`flex items-center gap-1 py-0.5 cursor-pointer select-none text-sm font-mono ${
          isSelected
            ? 'bg-blue-500/20 dark:bg-blue-600/30 ring-1 ring-inset ring-blue-500/50'
            : isFocused
            ? 'outline outline-2 outline-yellow-500/70 dark:outline-yellow-400/70 bg-gray-200/50 dark:bg-gray-700/50'
            : 'hover:bg-gray-100 dark:hover:bg-gray-700'
        }`}
        style={{ paddingLeft: `${depth * 16 + 8}px` }}
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        title={node.value_preview}
        data-node-id={node.id}
      >
        <span className="w-4 text-gray-400 dark:text-gray-500 flex-shrink-0 flex items-center justify-center">
          {node.has_children ? (
            isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />
          ) : null}
        </span>
        {node.key !== null && (
          <span className="text-gray-700 dark:text-gray-300 flex-shrink-0">{node.key}:&nbsp;</span>
        )}
        <span className={`${TYPE_COLORS[node.value_type] ?? 'text-gray-700 dark:text-gray-300'} truncate`}>
          {node.value_preview}
        </span>
        {node.has_children && (
          <span className="text-gray-400 dark:text-gray-600 text-xs ml-1 flex-shrink-0">
            ({node.children_count})
          </span>
        )}
      </div>
      {isExpanded && children && children.map((child) => (
        <TreeNode key={child.id} node={child} depth={depth + 1} />
      ))}

      {/* Context menu */}
      {contextMenu && (
        <div
          ref={menuRef}
          className="fixed z-50 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded shadow-lg py-1 text-sm text-gray-800 dark:text-gray-200 min-w-[160px]"
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <button
            className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
            onClick={copyPath}
          >
            Copia path
          </button>
          <button
            className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
            onClick={copyValue}
          >
            Copia valore
          </button>
          <button
            className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
            onClick={copyRaw}
          >
            Copia raw JSON
          </button>
        </div>
      )}
    </div>
  )
}

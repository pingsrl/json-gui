import { useRef, useEffect, useMemo, useCallback, type FC } from "react";
import { FolderOpen, ChevronDown } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useJsonStore,
  getParentId,
  getVisibleSlice,
  buildVisibleSubtreeSizeMap,
  countVisibleNodes,
  findVisibleNodeIndex
} from "../store";
import { useI18n } from "../i18n";
import { TreeNode } from "./TreeNode";

const SLICE_PADDING = 40;

export const TreePanel: FC = () => {
  const rootNode = useJsonStore((state) => state.rootNode);
  const rootChildren = useJsonStore((state) => state.rootChildren);
  const selectedNodeId = useJsonStore((state) => state.selectedNodeId);
  const focusedNodeId = useJsonStore((state) => state.focusedNodeId);
  const expandedNodes = useJsonStore((state) => state.expandedNodes);
  const expandAll = useJsonStore((state) => state.expandAll);
  const collapseAll = useJsonStore((state) => state.collapseAll);
  const toggleNode = useJsonStore((state) => state.toggleNode);
  const setFocusedNode = useJsonStore((state) => state.setFocusedNode);
  const { t } = useI18n();
  const treeRef = useRef<HTMLDivElement>(null);

  const subtreeSizeMap = useMemo(
    () => buildVisibleSubtreeSizeMap(rootChildren, expandedNodes),
    [rootChildren, expandedNodes]
  );
  const visibleCount = useMemo(
    () => countVisibleNodes(rootChildren, expandedNodes, subtreeSizeMap),
    [rootChildren, expandedNodes, subtreeSizeMap]
  );

  const rowVirtualizer = useVirtualizer({
    count: visibleCount,
    getScrollElement: () => treeRef.current,
    estimateSize: () => 24,
    overscan: 20
  });
  const virtualItems = rowVirtualizer.getVirtualItems();

  const sliceStart = virtualItems.length > 0
    ? Math.max(virtualItems[0].index - SLICE_PADDING, 0)
    : 0;
  const sliceEnd = virtualItems.length > 0
    ? Math.min(virtualItems[virtualItems.length - 1].index + SLICE_PADDING + 1, visibleCount)
    : Math.min(visibleCount, SLICE_PADDING * 2);
  const visibleSlice = useMemo(
    () =>
      getVisibleSlice(
        rootChildren,
        expandedNodes,
        sliceStart,
        sliceEnd - sliceStart,
        subtreeSizeMap
      ),
    [rootChildren, expandedNodes, sliceStart, sliceEnd, subtreeSizeMap]
  );
  const focusedIndexRef = useRef<{ nodeId: number | null; index: number }>({
    nodeId: null,
    index: -1
  });
  const treeStateRef = useRef({ rootChildren, expandedNodes });
  treeStateRef.current = { rootChildren, expandedNodes };

  const getVNodeAt = useCallback(
    (index: number) => {
      if (index < 0 || index >= visibleCount) return null;
      if (index >= sliceStart && index < sliceEnd) {
        return visibleSlice[index - sliceStart] ?? null;
      }
      return (
        getVisibleSlice(rootChildren, expandedNodes, index, 1, subtreeSizeMap)[0] ??
        null
      );
    },
    [
      expandedNodes,
      rootChildren,
      sliceEnd,
      sliceStart,
      subtreeSizeMap,
      visibleCount,
      visibleSlice
    ]
  );

  useEffect(() => {
    focusedIndexRef.current = {
      nodeId: focusedNodeId,
      index:
        focusedNodeId === null
          ? -1
          : findVisibleNodeIndex(rootChildren, expandedNodes, focusedNodeId)
    };
  }, [expandedNodes, focusedNodeId, rootChildren]);

  // Scroll al nodo selezionato — scatta SOLO quando selectedNodeId cambia
  useEffect(() => {
    if (selectedNodeId === null) return;
    const { rootChildren, expandedNodes } = treeStateRef.current;
    const idx = findVisibleNodeIndex(rootChildren, expandedNodes, selectedNodeId);
    if (idx >= 0) rowVirtualizer.scrollToIndex(idx, { align: "center" });
    focusedIndexRef.current = { nodeId: selectedNodeId, index: idx };
  }, [selectedNodeId, rowVirtualizer]);

  // Ref sempre aggiornato con i valori correnti — evita di ri-registrare il listener
  // ad ogni expand/collapse (da N dipendenze a 1)
  const kbStateRef = useRef({
    visibleCount,
    expandedNodes,
    toggleNode,
    setFocusedNode,
    rowVirtualizer,
    focusedNodeId,
    getVNodeAt,
    rootChildren
  });
  kbStateRef.current = {
    visibleCount,
    expandedNodes,
    toggleNode,
    setFocusedNode,
    rowVirtualizer,
    focusedNodeId,
    getVNodeAt,
    rootChildren
  };

  // Navigazione tastiera — ri-registrata solo quando cambia rootChildren (apertura file)
  useEffect(() => {
    if (rootChildren.length === 0) return;
    const handler = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if (
        !["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Enter"].includes(
          e.key
        )
      )
        return;
      e.preventDefault();

      const {
        visibleCount,
        expandedNodes,
        toggleNode,
        setFocusedNode,
        rowVirtualizer,
        focusedNodeId,
        getVNodeAt,
        rootChildren
      } = kbStateRef.current;

      if (visibleCount === 0) return;

      const idx =
        focusedNodeId !== null
          ? focusedIndexRef.current.nodeId === focusedNodeId
            ? focusedIndexRef.current.index
            : findVisibleNodeIndex(rootChildren, expandedNodes, focusedNodeId)
          : -1;
      if (focusedNodeId !== null) {
        focusedIndexRef.current = { nodeId: focusedNodeId, index: idx };
      }

      if (e.key === "ArrowDown") {
        const nextIdx = idx < visibleCount - 1 ? idx + 1 : 0;
        const next = getVNodeAt(nextIdx);
        if (next) {
          setFocusedNode(next.node.id);
          focusedIndexRef.current = { nodeId: next.node.id, index: nextIdx };
          rowVirtualizer.scrollToIndex(nextIdx, { align: "auto" });
        }
      } else if (e.key === "ArrowUp") {
        const prevIdx = idx > 0 ? idx - 1 : visibleCount - 1;
        const prev = getVNodeAt(prevIdx);
        if (prev) {
          setFocusedNode(prev.node.id);
          focusedIndexRef.current = { nodeId: prev.node.id, index: prevIdx };
          rowVirtualizer.scrollToIndex(prevIdx, { align: "auto" });
        }
      } else if (e.key === "ArrowRight") {
        const vNode = getVNodeAt(idx);
        if (vNode && vNode.node.children_count > 0 && !expandedNodes.has(vNode.node.id))
          toggleNode(vNode.node.id);
      } else if (e.key === "ArrowLeft") {
        const vNode = getVNodeAt(idx);
        if (!vNode) return;
        if (expandedNodes.has(vNode.node.id)) {
          toggleNode(vNode.node.id);
        } else {
          // O(1) via parentMap invece di O(n·m) nested loop su expandedNodes
          const parentId = getParentId(vNode.node.id);
          if (parentId !== undefined) {
            setFocusedNode(parentId);
            const parentIdx = findVisibleNodeIndex(rootChildren, expandedNodes, parentId);
            focusedIndexRef.current = { nodeId: parentId, index: parentIdx };
            if (parentIdx >= 0) {
              rowVirtualizer.scrollToIndex(parentIdx, { align: "auto" });
            }
          }
        }
      } else if (e.key === "Enter") {
        const vNode = getVNodeAt(idx);
        if (vNode && vNode.node.children_count > 0) toggleNode(vNode.node.id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [rootChildren]);

  return (
    <div className="flex-1 flex flex-col border-r border-gray-200 dark:border-gray-700 min-w-0">
      {rootChildren.length > 0 && (
        <div className="flex gap-1 px-2 py-1 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
          <button
            onClick={() => expandAll()}
            className="text-xs px-2 py-0.5 rounded text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700"
          >
            {t.expandAll}
          </button>
          <button
            onClick={() => collapseAll()}
            className="text-xs px-2 py-0.5 rounded text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700"
          >
            {t.collapseAll}
          </button>
        </div>
      )}
      {rootNode && (
        <div className="flex items-center gap-1 py-0.5 select-none text-sm font-mono border-b border-gray-100 dark:border-gray-800 flex-shrink-0"
          style={{ paddingLeft: "8px" }}
        >
          <span className="w-4 text-gray-400 dark:text-gray-500 flex-shrink-0 flex items-center justify-center">
            {rootChildren.length > 0 && <ChevronDown size={12} />}
          </span>
          <span className={`font-medium ${{
            array: "text-orange-600 dark:text-orange-400",
            object: "text-purple-600 dark:text-purple-400",
            string: "text-green-600 dark:text-green-400",
            number: "text-blue-600 dark:text-blue-400",
            boolean: "text-amber-600 dark:text-yellow-400",
            null: "text-gray-400 dark:text-gray-500",
          }[rootNode.value_type] ?? ""}`}>
            {rootNode.value_preview}
          </span>
        </div>
      )}
      <div ref={treeRef} className="flex-1 overflow-auto app-scrollbar">
        {!rootNode ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-400 dark:text-gray-500 gap-3">
            <FolderOpen size={40} className="opacity-30" />
            <span className="text-sm">{t.openJsonFile}</span>
            <span className="text-xs opacity-50">{t.anySize}</span>
          </div>
        ) : rootChildren.length === 0 ? null : (
          <div
            style={{
              height: `${rowVirtualizer.getTotalSize()}px`,
              position: "relative"
            }}
          >
            {virtualItems.map((vItem) => {
              const vNode = visibleSlice[vItem.index - sliceStart] ?? null;
              return (
                <div
                  key={vItem.key}
                  style={{
                    position: "absolute",
                    top: vItem.start,
                    height: 24,
                    width: "100%"
                  }}
                >
                  {vNode && <TreeNode node={vNode.node} depth={vNode.depth} />}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
};

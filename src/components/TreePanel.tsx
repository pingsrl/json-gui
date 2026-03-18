import { useRef, useEffect, useMemo, type FC } from "react";
import { FolderOpen, ChevronDown } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useJsonStore, getParentId } from "../store";
import { useI18n } from "../i18n";
import { TreeNode } from "./TreeNode";

export const TreePanel: FC = () => {
  const {
    rootNode,
    rootChildren,
    visibleNodes,
    expandAllActive,
    expandAllTotalCount,
    expandAllRows,
    selectedNodeId,
    focusedNodeId,
    expandedNodes,
    expandAll,
    collapseAll,
    fetchExpandedSlice,
    toggleNode,
    setFocusedNode
  } = useJsonStore();
  const { t } = useI18n();
  const treeRef = useRef<HTMLDivElement>(null);
  const visibleCount = expandAllActive ? expandAllTotalCount : visibleNodes.length;
  const getVNodeAt = (index: number) =>
    expandAllActive ? (expandAllRows.get(index) ?? null) : (visibleNodes[index] ?? null);

  const rowVirtualizer = useVirtualizer({
    count: visibleCount,
    getScrollElement: () => treeRef.current,
    estimateSize: () => 24,
    overscan: 20
  });
  const virtualItems = rowVirtualizer.getVirtualItems();
  const firstVirtualIndex = virtualItems[0]?.index ?? 0;
  const lastVirtualIndex =
    virtualItems.length > 0 ? virtualItems[virtualItems.length - 1].index : 0;

  // Indice O(1): nodeId → posizione in visibleNodes; ricalcolato solo su expand/collapse
  const visibleIndexMap = useMemo(() => {
    const map = new Map<number, number>();
    visibleNodes.forEach((vn, i) => map.set(vn.node.id, i));
    return map;
  }, [visibleNodes]);

  // Indice O(1): nodeId → posizione in expandAllRows; ricalcolato solo su nuovi slice
  const expandAllIndexMap = useMemo(() => {
    const map = new Map<number, number>();
    expandAllRows.forEach((vNode, idx) => map.set(vNode.node.id, idx));
    return map;
  }, [expandAllRows]);

  useEffect(() => {
    if (!expandAllActive || visibleCount === 0) return;
    const start = Math.max(0, firstVirtualIndex - 40);
    const limit = Math.max(1, lastVirtualIndex - start + 41);
    void fetchExpandedSlice(start, limit);
  }, [
    expandAllActive,
    visibleCount,
    firstVirtualIndex,
    lastVirtualIndex,
    fetchExpandedSlice
  ]);

  // Scroll al nodo selezionato — lookup O(1) via index map
  useEffect(() => {
    if (selectedNodeId === null) return;
    const idx = expandAllActive
      ? (expandAllIndexMap.get(selectedNodeId) ?? -1)
      : (visibleIndexMap.get(selectedNodeId) ?? -1);
    if (idx >= 0) rowVirtualizer.scrollToIndex(idx, { align: "center" });
  }, [
    expandAllActive,
    expandAllIndexMap,
    selectedNodeId,
    visibleIndexMap,
    rowVirtualizer
  ]);

  // Ref sempre aggiornato con i valori correnti — evita di ri-registrare il listener
  // ad ogni expand/collapse (da 12 dipendenze a 1)
  const kbStateRef = useRef({
    expandAllActive,
    expandAllIndexMap,
    visibleIndexMap,
    visibleCount,
    expandAllRows,
    visibleNodes,
    expandedNodes,
    fetchExpandedSlice,
    toggleNode,
    setFocusedNode,
    rowVirtualizer,
    focusedNodeId
  });
  kbStateRef.current = {
    expandAllActive,
    expandAllIndexMap,
    visibleIndexMap,
    visibleCount,
    expandAllRows,
    visibleNodes,
    expandedNodes,
    fetchExpandedSlice,
    toggleNode,
    setFocusedNode,
    rowVirtualizer,
    focusedNodeId
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
        expandAllActive,
        expandAllIndexMap,
        visibleIndexMap,
        visibleCount,
        expandAllRows,
        visibleNodes,
        expandedNodes,
        fetchExpandedSlice,
        toggleNode,
        setFocusedNode,
        rowVirtualizer,
        focusedNodeId
      } = kbStateRef.current;

      if (visibleCount === 0) return;

      const getVNodeAt = (i: number) =>
        expandAllActive ? (expandAllRows.get(i) ?? null) : (visibleNodes[i] ?? null);

      // Lookup O(1) via index map invece di findIndex O(n)
      const idx =
        focusedNodeId !== null
          ? expandAllActive
            ? (expandAllIndexMap.get(focusedNodeId) ?? -1)
            : (visibleIndexMap.get(focusedNodeId) ?? -1)
          : -1;

      if (e.key === "ArrowDown") {
        const nextIdx = idx < visibleCount - 1 ? idx + 1 : 0;
        const next = getVNodeAt(nextIdx);
        if (!next && expandAllActive) {
          void fetchExpandedSlice(nextIdx, 80);
          rowVirtualizer.scrollToIndex(nextIdx, { align: "auto" });
          return;
        }
        if (next) {
          setFocusedNode(next.node.id);
          rowVirtualizer.scrollToIndex(nextIdx, { align: "auto" });
        }
      } else if (e.key === "ArrowUp") {
        const prevIdx = idx > 0 ? idx - 1 : visibleCount - 1;
        const prev = getVNodeAt(prevIdx);
        if (!prev && expandAllActive) {
          void fetchExpandedSlice(prevIdx, 80);
          rowVirtualizer.scrollToIndex(prevIdx, { align: "auto" });
          return;
        }
        if (prev) {
          setFocusedNode(prev.node.id);
          rowVirtualizer.scrollToIndex(prevIdx, { align: "auto" });
        }
      } else if (expandAllActive) {
        return;
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
            const parentIdx = visibleIndexMap.get(parentId) ?? -1;
            if (parentIdx >= 0)
              rowVirtualizer.scrollToIndex(parentIdx, { align: "auto" });
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
              const vNode = getVNodeAt(vItem.index);
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
                  {vNode ? (
                    <TreeNode node={vNode.node} depth={vNode.depth} />
                  ) : (
                    <div className="h-6 mx-2 rounded bg-gray-100/70 dark:bg-gray-800/70" />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
};

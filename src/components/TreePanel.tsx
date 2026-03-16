import { useRef, useEffect, type FC } from "react";
import { FolderOpen } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";
import { TreeNode } from "./TreeNode";

export const TreePanel: FC = () => {
  const {
    rootChildren,
    visibleNodes,
    selectedNodeId,
    focusedNodeId,
    expandedNodes,
    expandAll,
    collapseAll,
    toggleNode,
    setFocusedNode
  } = useJsonStore();
  const { t } = useI18n();
  const treeRef = useRef<HTMLDivElement>(null);

  const rowVirtualizer = useVirtualizer({
    count: visibleNodes.length,
    getScrollElement: () => treeRef.current,
    estimateSize: () => 24,
    overscan: 20
  });

  // Scroll al nodo selezionato
  useEffect(() => {
    if (selectedNodeId === null) return;
    const idx = visibleNodes.findIndex(
      ({ node }) => node.id === selectedNodeId
    );
    if (idx >= 0) rowVirtualizer.scrollToIndex(idx, { align: "center" });
  }, [selectedNodeId, visibleNodes, rowVirtualizer]);

  // Navigazione tastiera
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

      const idx =
        focusedNodeId !== null
          ? visibleNodes.findIndex(({ node }) => node.id === focusedNodeId)
          : -1;

      if (e.key === "ArrowDown") {
        const next =
          idx < visibleNodes.length - 1
            ? visibleNodes[idx + 1]
            : visibleNodes[0];
        if (next) {
          setFocusedNode(next.node.id);
          rowVirtualizer.scrollToIndex(
            idx < visibleNodes.length - 1 ? idx + 1 : 0,
            { align: "auto" }
          );
        }
      } else if (e.key === "ArrowUp") {
        const prevIdx = idx > 0 ? idx - 1 : visibleNodes.length - 1;
        const prev = visibleNodes[prevIdx];
        if (prev) {
          setFocusedNode(prev.node.id);
          rowVirtualizer.scrollToIndex(prevIdx, { align: "auto" });
        }
      } else if (e.key === "ArrowRight") {
        const vNode = visibleNodes[idx];
        if (vNode?.node.has_children && !expandedNodes.has(vNode.node.id))
          toggleNode(vNode.node.id);
      } else if (e.key === "ArrowLeft") {
        const vNode = visibleNodes[idx];
        if (!vNode) return;
        if (expandedNodes.has(vNode.node.id)) {
          toggleNode(vNode.node.id);
        } else {
          for (const [parentId, children] of expandedNodes.entries()) {
            if (children.some((c) => c.id === vNode.node.id)) {
              setFocusedNode(parentId);
              const parentIdx = visibleNodes.findIndex(
                ({ node }) => node.id === parentId
              );
              if (parentIdx >= 0)
                rowVirtualizer.scrollToIndex(parentIdx, { align: "auto" });
              break;
            }
          }
        }
      } else if (e.key === "Enter") {
        const vNode = visibleNodes[idx];
        if (vNode?.node.has_children) toggleNode(vNode.node.id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [
    focusedNodeId,
    visibleNodes,
    expandedNodes,
    rootChildren,
    toggleNode,
    setFocusedNode,
    rowVirtualizer
  ]);

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
      <div ref={treeRef} className="flex-1 overflow-auto">
        {rootChildren.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-400 dark:text-gray-500 gap-3">
            <FolderOpen size={40} className="opacity-30" />
            <span className="text-sm">{t.openJsonFile}</span>
            <span className="text-xs opacity-50">{t.anySize}</span>
          </div>
        ) : (
          <div
            style={{
              height: `${rowVirtualizer.getTotalSize()}px`,
              position: "relative"
            }}
          >
            {rowVirtualizer.getVirtualItems().map((vItem) => {
              const vNode = visibleNodes[vItem.index];
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
                  <TreeNode node={vNode.node} depth={vNode.depth} />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
};

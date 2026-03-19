import { type FC } from "react";
import { NodeDto, useJsonStore, getParentId } from "../store";
import { ChevronRight, ChevronDown } from "lucide-react";

const TYPE_COLORS: Record<string, string> = {
  string: "text-green-600 dark:text-green-400",
  number: "text-blue-600 dark:text-blue-400",
  boolean: "text-amber-600 dark:text-yellow-400",
  null: "text-gray-400 dark:text-gray-500",
  object: "text-purple-600 dark:text-purple-400",
  array: "text-orange-600 dark:text-orange-400",
  "load-more": "text-sky-600 dark:text-sky-400"
};

interface Props {
  node: NodeDto;
  depth: number;
}

export const TreeNode: FC<Props> = ({ node, depth }) => {
  const {
    expandedNodes,
    toggleNode,
    loadMoreChildren,
    selectNode,
    selectedNodeId,
    focusedNodeId,
    showContextMenu
  } = useJsonStore();
  const isLoadMore = node.synthetic_kind === "load-more";
  const hasChildren = node.children_count > 0;
  const isExpanded = expandedNodes.has(node.id);
  const isSelected = !isLoadMore && selectedNodeId === node.id;
  const isFocused = !isLoadMore && focusedNodeId === node.id;

  const handleSelect = () => {
    if (isLoadMore) {
      if (node.parent_node_id !== undefined && node.next_offset !== undefined) {
        void loadMoreChildren(node.parent_node_id, node.next_offset);
      }
      return;
    }
    void selectNode(node);
  };

  const handleToggle = (e?: React.MouseEvent) => {
    e?.stopPropagation();
    if (isLoadMore) {
      handleSelect();
      return;
    }
    if (!hasChildren) return;
    void toggleNode(node.id);
  };

  const handleDoubleClick = () => {
    if (isLoadMore) {
      handleSelect();
      return;
    }
    handleSelect();
    if (hasChildren) {
      void toggleNode(node.id);
    }
  };

  const handleContextMenu = (e: React.MouseEvent) => {
    if (isLoadMore) return;
    e.preventDefault();
    e.stopPropagation();
    void selectNode(node);
    showContextMenu({
      x: e.clientX,
      y: e.clientY,
      nodeId: node.id,
      parentId: getParentId(node.id) ?? null,
      nodeKey: node.key,
      valueType: node.value_type
    });
  };

  return (
    <div
      id={`node-${node.id}`}
      className={`flex items-center gap-1 py-0.5 cursor-pointer select-none text-sm font-mono ${
        isSelected
          ? "bg-blue-500/20 dark:bg-blue-600/30 ring-1 ring-inset ring-blue-500/50"
          : isFocused
            ? "outline outline-2 outline-yellow-500/70 dark:outline-yellow-400/70 bg-gray-200/50 dark:bg-gray-700/50"
            : "hover:bg-gray-100 dark:hover:bg-gray-700"
      }`}
      style={{ paddingLeft: `${depth * 16 + 8}px` }}
      onClick={handleSelect}
      onDoubleClick={handleDoubleClick}
      onContextMenu={isLoadMore ? undefined : handleContextMenu}
      title={node.value_preview}
      data-node-id={node.id}
    >
      <button
        type="button"
        aria-label={
          hasChildren
            ? isExpanded
              ? "Collapse node"
              : "Expand node"
            : "Leaf node"
        }
        className="w-4 text-gray-400 dark:text-gray-500 flex-shrink-0 flex items-center justify-center disabled:cursor-default"
        disabled={!hasChildren && !isLoadMore}
        onClick={handleToggle}
      >
        {hasChildren ? (
          isExpanded ? (
            <ChevronDown size={12} />
          ) : (
            <ChevronRight size={12} />
          )
        ) : null}
      </button>
      {node.key !== null && (
        <span className="text-gray-700 dark:text-gray-300 flex-shrink-0">
          {node.key}:&nbsp;
        </span>
      )}
      <span
        className={`${TYPE_COLORS[node.value_type] ?? "text-gray-700 dark:text-gray-300"} truncate`}
      >
        {node.value_preview}
      </span>
      {hasChildren && !isLoadMore && (
        <span className="text-gray-400 dark:text-gray-600 text-xs ml-1 flex-shrink-0">
          ({node.children_count})
        </span>
      )}
    </div>
  );
};

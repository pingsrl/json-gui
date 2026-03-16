import { type FC } from "react";
import { type NodeDto, useJsonStore } from "../store";

const TYPE_BADGE: Record<string, string> = {
  string:
    "bg-green-100  text-green-700  dark:bg-green-900/40  dark:text-green-400",
  number:
    "bg-blue-100   text-blue-700   dark:bg-blue-900/40   dark:text-blue-400",
  boolean:
    "bg-amber-100  text-amber-700  dark:bg-amber-900/40  dark:text-amber-400",
  null: "bg-gray-100   text-gray-500   dark:bg-gray-700      dark:text-gray-400",
  object:
    "bg-purple-100 text-purple-700 dark:bg-purple-900/40 dark:text-purple-400",
  array:
    "bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-400"
};

function Row({
  label,
  children
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="px-3 py-2 border-b border-gray-100 dark:border-gray-800">
      <div className="text-xs text-gray-400 dark:text-gray-500 mb-0.5">
        {label}
      </div>
      <div className="text-xs font-mono text-gray-800 dark:text-gray-200 break-all">
        {children}
      </div>
    </div>
  );
}

function NodeRow({
  node,
  isSelected,
  onClick
}: {
  node: NodeDto;
  isSelected: boolean;
  onClick: () => void;
}) {
  const badge = TYPE_BADGE[node.value_type] ?? TYPE_BADGE.null;
  return (
    <div
      onClick={onClick}
      className={`px-3 py-1.5 border-b border-gray-100 dark:border-gray-800 cursor-pointer flex items-center gap-1.5 min-w-0 ${
        isSelected
          ? "bg-blue-50 dark:bg-blue-900/20"
          : "hover:bg-gray-50 dark:hover:bg-gray-800/50"
      }`}
    >
      {node.key !== null && (
        <span
          className={`text-xs font-mono flex-shrink-0 ${
            isSelected
              ? "text-blue-700 dark:text-blue-300 font-semibold"
              : "text-gray-700 dark:text-gray-300"
          }`}
        >
          {node.key}
        </span>
      )}
      <span className={`text-xs px-1 py-px rounded flex-shrink-0 ${badge}`}>
        {node.value_type}
      </span>
      <span className="text-xs font-mono text-gray-500 dark:text-gray-400 truncate">
        {node.value_preview}
      </span>
    </div>
  );
}

export const PropertiesPanel: FC = () => {
  const {
    selectedNode,
    selectedNodePath,
    expandedNodes,
    navigateToNode
  } = useJsonStore();

  if (!selectedNode) {
    return (
      <div className="flex items-center justify-center h-full text-gray-400 dark:text-gray-600 text-xs px-4 text-center">
        Seleziona un nodo per vederne le proprietà
      </div>
    );
  }

  const badge = TYPE_BADGE[selectedNode.value_type] ?? TYPE_BADGE.null;
  const ownChildren = expandedNodes.get(selectedNode.id);

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 flex-shrink-0">
        <span className={`text-xs font-medium px-1.5 py-0.5 rounded ${badge}`}>
          {selectedNode.value_type}
        </span>
        <span className="text-sm font-mono text-gray-800 dark:text-gray-200 truncate">
          {selectedNode.key ?? "(root)"}
        </span>
      </div>

      <div className="overflow-auto flex-1 flex flex-col">
        {/* Path */}
        {selectedNodePath && (
          <Row label="Path">
            <span className="text-blue-600 dark:text-blue-400">
              {selectedNodePath}
            </span>
          </Row>
        )}

        {/* Valore */}
        {selectedNode.value_type !== "object" &&
          selectedNode.value_type !== "array" && (
            <Row label="Valore">{selectedNode.value_preview}</Row>
          )}

        {/* Dimensione per object/array */}
        {(selectedNode.value_type === "object" ||
          selectedNode.value_type === "array") && (
          <Row
            label={selectedNode.value_type === "object" ? "Chiavi" : "Elementi"}
          >
            {selectedNode.children_count.toLocaleString()}
          </Row>
        )}

        {/* Figli espansi */}
        {ownChildren && ownChildren.length > 0 && (
          <div className="flex flex-col min-h-0">
            <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-100 dark:border-gray-800 bg-white dark:bg-gray-900 flex-shrink-0">
              Figli ({ownChildren.length})
            </div>
            {ownChildren.map((child) => (
              <NodeRow
                key={child.id}
                node={child}
                isSelected={false}
                onClick={() => navigateToNode(child.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
};

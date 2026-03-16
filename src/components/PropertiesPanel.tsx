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

const VALUE_COLORS: Record<string, string> = {
  string: "text-green-600 dark:text-green-400",
  number: "text-blue-600 dark:text-blue-400",
  boolean: "text-amber-600 dark:text-yellow-400",
  null: "text-gray-400 dark:text-gray-500",
  object: "text-purple-600 dark:text-purple-400",
  array: "text-orange-600 dark:text-orange-400"
};

function NodeRow({
  node,
  isSelected,
  onClick
}: {
  node: NodeDto;
  isSelected: boolean;
  onClick: () => void;
}) {
  const valueColor = VALUE_COLORS[node.value_type] ?? VALUE_COLORS.null;
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
          {node.key}:
        </span>
      )}
      <span className={`text-xs font-mono truncate ${valueColor}`}>
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
    rootChildren,
    navigateToNode
  } = useJsonStore();

  // Trova i fratelli del nodo selezionato
  let siblings: NodeDto[] | null = null;
  if (selectedNode) {
    for (const children of expandedNodes.values()) {
      if (children.some((c) => c.id === selectedNode.id)) {
        siblings = children;
        break;
      }
    }
    if (!siblings && rootChildren.some((c) => c.id === selectedNode.id)) {
      siblings = rootChildren;
    }
  }

  const badge = selectedNode
    ? (TYPE_BADGE[selectedNode.value_type] ?? TYPE_BADGE.null)
    : null;
  const ownChildren = selectedNode ? expandedNodes.get(selectedNode.id) : null;

  // Calcola il nome del genitore dal path
  const pathParts = selectedNodePath?.split(".") ?? [];
  const parentKey = pathParts.length > 1 ? pathParts[pathParts.length - 2] : null;

  return (
    <div className="flex flex-col h-full overflow-hidden">

      {/* ── Sezione fratelli (flex-1, sempre visibile) ── */}
      <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
        <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex-shrink-0">
          {siblings && siblings.length > 0 ? (
            <>
              {parentKey ? (
                <span className="font-mono text-gray-600 dark:text-gray-400">{parentKey}</span>
              ) : "Oggetto padre"}{" "}
              <span className="text-gray-400 dark:text-gray-600">({siblings.length})</span>
            </>
          ) : (
            "Fratelli"
          )}
        </div>
        <div className="flex-1 overflow-auto">
          {siblings && siblings.length > 0 ? (
            siblings.map((sib) => (
              <NodeRow
                key={sib.id}
                node={sib}
                isSelected={sib.id === selectedNode!.id}
                onClick={() => navigateToNode(sib.id)}
              />
            ))
          ) : (
            <div className="p-3 text-gray-400 dark:text-gray-600 text-xs">
              {selectedNode ? "Nessun fratello" : "Seleziona un nodo"}
            </div>
          )}
        </div>
      </div>

      {/* ── Sezione proprietà (1/5 del totale, max 400px) ── */}
      <div
        className="flex-shrink-0 border-t border-gray-200 dark:border-gray-700 overflow-auto"
        style={{ maxHeight: "min(20%, 400px)" }}
      >
        {selectedNode ? (
          <>
            {/* Header */}
            <div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 sticky top-0 bg-white dark:bg-gray-900">
              <span className={`text-xs font-medium px-1.5 py-0.5 rounded ${badge}`}>
                {selectedNode.value_type}
              </span>
              <span className="text-sm font-mono text-gray-800 dark:text-gray-200 truncate">
                {selectedNode.key ?? "(root)"}
              </span>
            </div>

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
              <>
                <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-100 dark:border-gray-800">
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
              </>
            )}
          </>
        ) : (
          <div className="p-3 text-gray-400 dark:text-gray-600 text-xs">
            Proprietà
          </div>
        )}
      </div>
    </div>
  );
};

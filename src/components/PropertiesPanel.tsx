import { type FC, useState, useCallback } from "react";
import { type NodeDto, useJsonStore } from "../store";
import { useI18n } from "../i18n";
import { DetailPanel } from "./DetailPanel";
import { ResizeHandle } from "./ResizeHandle";

const MIN_DETAIL = 80;
const MAX_DETAIL = 600;
const DEFAULT_DETAIL = 220;

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
    selectedNodeSiblings,
    navigateToNode
  } = useJsonStore();
  const { t } = useI18n();

  const [detailHeight, setDetailHeight] = useState(() => {
    const saved = localStorage.getItem("panel-detail-height");
    return saved ? parseInt(saved, 10) : DEFAULT_DETAIL;
  });

  const handleDetailResize = useCallback((delta: number) => {
    setDetailHeight((h) => {
      const next = Math.max(MIN_DETAIL, Math.min(MAX_DETAIL, h - delta));
      localStorage.setItem("panel-detail-height", String(next));
      return next;
    });
  }, []);

  const siblings = selectedNode ? selectedNodeSiblings : null;

  // Calcola il nome del genitore dal path
  const pathParts = selectedNodePath?.split(".") ?? [];
  const parentKey =
    pathParts.length > 1 ? pathParts[pathParts.length - 2] : null;

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* ── Sezione fratelli (flex-1) ── */}
      <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
        <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex-shrink-0">
          {siblings && siblings.length > 0 ? (
            <>
              {parentKey ? (
                <span className="font-mono text-gray-600 dark:text-gray-400">
                  {parentKey}
                </span>
              ) : (
                t.parentObject
              )}{" "}
              <span className="text-gray-400 dark:text-gray-600">
                ({siblings.length})
              </span>
            </>
          ) : (
            t.siblings
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
              {selectedNode ? t.noSiblings : t.selectNode}
            </div>
          )}
        </div>
      </div>

      <ResizeHandle direction="vertical" onResize={handleDetailResize} />

      <div
        style={{ height: detailHeight }}
        className="flex-shrink-0 overflow-hidden"
      >
        <DetailPanel />
      </div>
    </div>
  );
};

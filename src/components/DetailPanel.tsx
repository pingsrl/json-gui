import { FC, useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Copy } from "lucide-react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

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
    <div className="px-3 py-2 border-b border-gray-100 dark:border-gray-800 select-none">
      <div className="text-xs text-gray-400 dark:text-gray-500 mb-0.5 select-none">
        {label}
      </div>
      <div className="text-xs font-mono text-gray-800 dark:text-gray-200 break-all select-text">
        {children}
      </div>
    </div>
  );
}

const SCALAR_TYPES = new Set(["string", "number", "boolean", "null"]);

function formatScalarValue(value: string | null, valueType: string): string | null {
  if (value === null || valueType !== "string") return value;
  try {
    const parsed = JSON.parse(value);
    return typeof parsed === "string" ? parsed : value;
  } catch {
    if (value.startsWith('"') && value.endsWith('"')) {
      return value.slice(1, -1);
    }
    return value;
  }
}

export const DetailPanel: FC = () => {
  const selectedNode = useJsonStore((state) => state.selectedNode);
  const selectedNodePath = useJsonStore((state) => state.selectedNodePath);
  const { t } = useI18n();
  const [fullValue, setFullValue] = useState<string | null>(null);

  useEffect(() => {
    if (!selectedNode || !SCALAR_TYPES.has(selectedNode.value_type)) {
      setFullValue(null);
      return;
    }
    invoke<string>("get_raw", { nodeId: selectedNode.id })
      .then(setFullValue)
      .catch(() => setFullValue(null));
  }, [selectedNode?.id]);

  const badge = selectedNode
    ? (TYPE_BADGE[selectedNode.value_type] ?? TYPE_BADGE.null)
    : null;
  const displayValue = selectedNode
    ? formatScalarValue(fullValue ?? selectedNode.value_preview, selectedNode.value_type)
    : null;

  const handleCopyValue = async () => {
    if (!selectedNode) return;
    try {
      if (SCALAR_TYPES.has(selectedNode.value_type)) {
        await navigator.clipboard.writeText(displayValue ?? "");
        return;
      }
      const raw = await invoke<string>("get_raw", { nodeId: selectedNode.id });
      await navigator.clipboard.writeText(raw);
    } catch (err) {
      console.error("detail copyValue error:", err);
    }
  };

  /* ── Sezione proprietà (1/5 del totale, max 500px) ── */
  return (
    <div className="h-full border-t border-gray-200 dark:border-gray-700 overflow-auto app-scrollbar">
      {selectedNode ? (
        <>
          {/* Header */}
          <div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 sticky top-0 bg-white dark:bg-gray-900">
            <span
              className={`text-xs font-medium px-1.5 py-0.5 rounded ${badge}`}
            >
              {selectedNode.value_type}
            </span>
            <span className="text-sm font-mono text-gray-800 dark:text-gray-200 truncate">
              {selectedNode.key ?? "(root)"}
            </span>
            <button
              onClick={handleCopyValue}
              className="ml-auto inline-flex h-7 w-7 items-center justify-center rounded text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-700 dark:text-gray-400 dark:hover:bg-gray-800 dark:hover:text-gray-200"
              title={t.copyValue}
              aria-label={t.copyValue}
            >
              <Copy size={14} />
            </button>
          </div>

          {/* Path */}
          {selectedNodePath && (
            <Row label={t.path}>
              <span className="text-blue-600 dark:text-blue-400">
                {selectedNodePath}
              </span>
            </Row>
          )}

          {/* Valore */}
          {SCALAR_TYPES.has(selectedNode.value_type) && (
            <Row label={t.value}>{displayValue}</Row>
          )}

          {/* Dimensione per object/array */}
          {(selectedNode.value_type === "object" ||
            selectedNode.value_type === "array") && (
            <Row
              label={selectedNode.value_type === "object" ? t.keys : t.elements}
            >
              {selectedNode.children_count.toLocaleString()}
            </Row>
          )}
        </>
      ) : (
        <div className="p-3 text-gray-400 dark:text-gray-600 text-xs">
          {t.propertiesPlaceholder}
        </div>
      )}
    </div>
  );
};

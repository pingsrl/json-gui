import { type FC } from "react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";
import { formatBytes } from "../utils";

export const StatusBar: FC = () => {
  const { nodeCount, sizeBytes, selectedNodePath, filePath } = useJsonStore();
  const { t } = useI18n();

  return (
    <div className="flex items-center gap-4 px-3 py-1 bg-white dark:bg-gray-800 border-t border-gray-200 dark:border-gray-700 text-xs text-gray-400 dark:text-gray-500 flex-shrink-0">
      <span>{t.nodes(nodeCount.toLocaleString())}</span>
      <span>{t.size(formatBytes(sizeBytes))}</span>
      {selectedNodePath && (
        <span
          className="font-mono text-blue-600 dark:text-blue-400 truncate flex-1"
          title={selectedNodePath}
        >
          {selectedNodePath}
        </span>
      )}
      {!selectedNodePath && filePath && (
        <span className="truncate flex-1 text-right" title={filePath}>
          {filePath}
        </span>
      )}
    </div>
  );
};

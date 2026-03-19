import { type FC, useEffect } from "react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";
import { formatBytes } from "../utils";

function formatCpuPercent(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "0.0%";
  return `${value.toFixed(1)}%`;
}

function formatDuration(valueMs: number): string {
  if (valueMs >= 1000) return `${(valueMs / 1000).toFixed(2)}s`;
  return `${Math.round(valueMs)}ms`;
}

export const StatusBar: FC = () => {
  const {
    nodeCount,
    sizeBytes,
    runtimeStats,
    lastOperation,
    refreshRuntimeStats
  } = useJsonStore();
  const { t } = useI18n();

  useEffect(() => {
    void refreshRuntimeStats();
    const timer = window.setInterval(() => {
      void refreshRuntimeStats();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [refreshRuntimeStats]);

  const liveRam = runtimeStats ? formatBytes(runtimeStats.resident_bytes) : "0 B";
  const liveCpu = formatCpuPercent(runtimeStats?.cpu_percent);
  const lastOperationSummary = lastOperation
    ? `${lastOperation.label} ${formatDuration(lastOperation.duration_ms)}`
    : t.noOperation;

  return (
    <div className="flex items-center gap-4 px-3 py-1 bg-white dark:bg-gray-800 border-t border-gray-200 dark:border-gray-700 text-xs text-gray-400 dark:text-gray-500 flex-shrink-0 overflow-hidden">
      <span>{t.nodes(nodeCount.toLocaleString())}</span>
      <span>{t.size(formatBytes(sizeBytes))}</span>
      <span>{t.ram(liveRam)}</span>
      <span>{t.cpu(liveCpu)}</span>
      <span className="truncate flex-1 text-right" title={lastOperationSummary}>
        {t.lastOperation}: {lastOperationSummary}
      </span>
    </div>
  );
};

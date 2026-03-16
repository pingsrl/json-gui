import { type FC, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

export const ContextMenu: FC = () => {
  const { contextMenu, hideContextMenu } = useJsonStore();
  const { t } = useI18n();
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!contextMenu) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        hideContextMenu();
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [contextMenu, hideContextMenu]);

  if (!contextMenu) return null;

  const { x, y, nodeId, valueType, valuePreview } = contextMenu;

  const copyPath = async () => {
    try {
      // Usa pathCache tramite selectNode oppure chiama direttamente
      const path = await invoke<string>("get_path", { nodeId });
      await navigator.clipboard.writeText(path);
    } catch (err) {
      console.error("copyPath error:", err);
    }
    hideContextMenu();
  };

  const copyValue = async () => {
    try {
      if (valueType === "object" || valueType === "array") {
        const raw = await invoke<string>("get_raw", { nodeId });
        await navigator.clipboard.writeText(raw);
      } else {
        await navigator.clipboard.writeText(valuePreview);
      }
    } catch (err) {
      console.error("copyValue error:", err);
    }
    hideContextMenu();
  };

  const copyRaw = async () => {
    try {
      const raw = await invoke<string>("get_raw", { nodeId });
      const pretty = JSON.stringify(JSON.parse(raw), null, 2);
      await navigator.clipboard.writeText(pretty);
    } catch (err) {
      console.error("copyRaw error:", err);
    }
    hideContextMenu();
  };

  return (
    <div
      ref={menuRef}
      className="fixed z-50 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded shadow-lg py-1 text-sm text-gray-800 dark:text-gray-200 min-w-[160px]"
      style={{ left: x, top: y }}
    >
      <button
        className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
        onClick={copyPath}
      >
        {t.copyPath}
      </button>
      <button
        className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
        onClick={copyValue}
      >
        {t.copyValue}
      </button>
      <button
        className="w-full text-left px-3 py-1.5 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
        onClick={copyRaw}
      >
        {t.copyRaw}
      </button>
    </div>
  );
};

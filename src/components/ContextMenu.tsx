import { type FC, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

export const ContextMenu: FC = () => {
  const { contextMenu, hideContextMenu, setSearchScopePath, expandSubtree } = useJsonStore();
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

  const { x, y, nodeId, parentId, nodeKey, valueType } = contextMenu;
  const hasNodeKey = nodeKey !== null;
  const hasParentNode = parentId !== null;
  const isContainer = valueType === "object" || valueType === "array";
  const itemClassName =
    "w-full text-left px-3 py-1.5 transition-colors hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent dark:disabled:hover:bg-transparent";

  const focusSearchInput = () => {
    requestAnimationFrame(() => {
      document.getElementById("primary-search-input")?.focus();
    });
  };

  const copyKey = async () => {
    if (!hasNodeKey) {
      hideContextMenu();
      return;
    }
    try {
      await navigator.clipboard.writeText(nodeKey);
    } catch (err) {
      console.error("copyKey error:", err);
    }
    hideContextMenu();
  };

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
      const raw = await invoke<string>("get_raw", { nodeId });
      if (valueType === "string") {
        await navigator.clipboard.writeText(JSON.parse(raw));
      } else {
        await navigator.clipboard.writeText(raw);
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

  const searchInScope = async (scopeNodeId: number | null) => {
    if (scopeNodeId === null) {
      hideContextMenu();
      return;
    }
    try {
      const path = await invoke<string>("get_path", { nodeId: scopeNodeId });
      setSearchScopePath(path);
      focusSearchInput();
    } catch (err) {
      console.error("searchInScope error:", err);
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
        type="button"
        className={itemClassName}
        onClick={() => searchInScope(nodeId)}
      >
        {t.searchInNode}
      </button>
      <button
        type="button"
        className={itemClassName}
        onClick={() => searchInScope(parentId)}
        disabled={!hasParentNode}
      >
        {t.searchInParentNode}
      </button>
      {isContainer && (
        <>
          <div className="my-1 border-t border-gray-200 dark:border-gray-700" />
          <button
            type="button"
            className={itemClassName}
            onClick={() => {
              void expandSubtree(nodeId);
              hideContextMenu();
            }}
          >
            {t.expandFromHere}
          </button>
        </>
      )}
      <div className="my-1 border-t border-gray-200 dark:border-gray-700" />
      <button
        type="button"
        className={itemClassName}
        onClick={copyKey}
        disabled={!hasNodeKey}
      >
        {t.copyKey}
      </button>
      <button
        type="button"
        className={itemClassName}
        onClick={copyPath}
      >
        {t.copyPath}
      </button>
      <button
        type="button"
        className={itemClassName}
        onClick={copyValue}
      >
        {t.copyValue}
      </button>
      <button
        type="button"
        className={itemClassName}
        onClick={copyRaw}
      >
        {t.copyRaw}
      </button>
    </div>
  );
};

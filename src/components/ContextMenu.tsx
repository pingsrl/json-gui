import { type FC, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

export const ContextMenu: FC = () => {
  const contextMenu = useJsonStore((state) => state.contextMenu);
  const hideContextMenu = useJsonStore((state) => state.hideContextMenu);
  const setSearchScopePath = useJsonStore((state) => state.setSearchScopePath);
  const expandSubtree = useJsonStore((state) => state.expandSubtree);
  const { t } = useI18n();
  const menuRef = useRef<HTMLDivElement>(null);
  // Pre-carica path e raw non appena il menu si apre, così le funzioni
  // di copia non devono fare await prima di writeText (il user-gesture token
  // in WKWebView scade dopo il primo await asincrono).
  const prefetchRef = useRef<{ path: string; raw: string } | null>(null);

  useEffect(() => {
    if (!contextMenu) { prefetchRef.current = null; return; }
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        hideContextMenu();
      }
    };
    document.addEventListener("mousedown", handler);
    // Pre-fetch in background: path + raw per quando l'utente clicca Copia
    const { nodeId } = contextMenu;
    prefetchRef.current = null;
    Promise.all([
      invoke<string>("get_path", { nodeId }),
      invoke<string>("get_raw", { nodeId }),
    ]).then(([path, raw]) => { prefetchRef.current = { path, raw }; });
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

  const write = (text: string) => navigator.clipboard.writeText(text).catch(console.error);

  const copyKey = () => {
    if (hasNodeKey) void write(nodeKey);
    hideContextMenu();
  };

  const copyPath = async () => {
    const text = prefetchRef.current?.path ?? await invoke<string>("get_path", { nodeId });
    void write(text);
    hideContextMenu();
  };

  const copyValue = async () => {
    const raw = prefetchRef.current?.raw ?? await invoke<string>("get_raw", { nodeId });
    void write(valueType === "string" ? JSON.parse(raw) as string : raw);
    hideContextMenu();
  };

  const copyRaw = async () => {
    const raw = prefetchRef.current?.raw ?? await invoke<string>("get_raw", { nodeId });
    void write(JSON.stringify(JSON.parse(raw), null, 2));
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
        onClick={() => {
          void invoke("open_in_new_window", { nodeId });
          hideContextMenu();
        }}
      >
        {t.openInNewWindow}
      </button>
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

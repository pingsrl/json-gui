import { useState, useEffect, useCallback } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { useJsonStore } from "./store";
import { useI18n } from "./i18n";
import { Toolbar } from "./components/Toolbar";
import { SearchPanel } from "./components/SearchPanel";
import { TreePanel } from "./components/TreePanel";
import { StatusBar } from "./components/StatusBar";
import { ContextMenu } from "./components/ContextMenu";
import { PropertiesPanel } from "./components/PropertiesPanel";
import { ResizeHandle } from "./components/ResizeHandle";
import { ExportModal } from "./components/ExportModal";

const MIN_PANEL = 160;
const MAX_PANEL = 600;
const DEFAULT_LEFT = 288;
const DEFAULT_RIGHT = 288;

export default function App() {
  const { filePath, openFile, openFromString } = useJsonStore();

  const [darkMode, setDarkMode] = useState(
    () => localStorage.getItem("theme") !== "light"
  );

  const [leftWidth, setLeftWidth] = useState(() => {
    const s = localStorage.getItem("panel-left-width");
    return s ? parseInt(s, 10) : DEFAULT_LEFT;
  });
  const [rightWidth, setRightWidth] = useState(() => {
    const s = localStorage.getItem("panel-right-width");
    return s ? parseInt(s, 10) : DEFAULT_RIGHT;
  });

  const handleLeftResize = useCallback((delta: number) => {
    setLeftWidth((w) => {
      const next = Math.max(MIN_PANEL, Math.min(MAX_PANEL, w + delta));
      localStorage.setItem("panel-left-width", String(next));
      return next;
    });
  }, []);

  const handleRightResize = useCallback((delta: number) => {
    setRightWidth((w) => {
      const next = Math.max(MIN_PANEL, Math.min(MAX_PANEL, w - delta));
      localStorage.setItem("panel-right-width", String(next));
      return next;
    });
  }, []);
  const [parseProgress, setParseProgress] = useState<number | null>(null);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const [pasteError, setPasteError] = useState<string | null>(null);
  const [updateToast, setUpdateToast] = useState<string | null>(null);
  const [showExport, setShowExport] = useState(false);

  // Dark mode
  useEffect(() => {
    document.documentElement.classList.toggle("dark", darkMode);
    document.documentElement.classList.toggle("light", !darkMode);
    localStorage.setItem("theme", darkMode ? "dark" : "light");
  }, [darkMode]);

  // Progress events dal backend (file >200MB)
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<number>("parse-progress", (e) => setParseProgress(e.payload)).then(
      (fn) => {
        unlisten = fn;
      }
    );
    return () => unlisten?.();
  }, []);

  const { loading } = useJsonStore();
  useEffect(() => {
    if (!loading) {
      const timer = setTimeout(() => setParseProgress(null), 400);
      return () => clearTimeout(timer);
    }
  }, [loading]);

  // Apri file passato come argomento CLI (Windows/Linux)
  useEffect(() => {
    invoke<string | null>("get_initial_path").then((path) => {
      if (path) openFile(path);
    });
  }, [openFile]);

  // Apri file via "Apri con" / double-click (macOS RunEvent::Opened)
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<string>("open-with", (e) => openFile(e.payload)).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [openFile]);

  // Controlla aggiornamenti all'avvio (silenzioso)
  useEffect(() => {
    check()
      .then((update) => {
        if (update?.available) setUpdateAvailable(true);
      })
      .catch(() => {});
  }, []);

  const handleOpenFile = async () => {
    const selected = await open({
      filters: [{ name: "JSON", extensions: ["json"] }]
    });
    if (selected) await openFile(selected as string);
  };

  const handleUpdate = async () => {
    setUpdating(true);
    try {
      const update = await check();
      if (update?.available) {
        await update.downloadAndInstall();
        await relaunch();
      }
    } catch (err) {
      console.error("Update failed:", err);
      setUpdating(false);
    }
  };

  // Cmd+F / Cmd+O / Cmd+R
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        document.getElementById("search-input")?.focus();
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "o") {
        e.preventDefault();
        handleOpenFile();
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "r" && filePath) {
        e.preventDefault();
        openFile(filePath);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath]);

  // Eventi menu nativo
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    Promise.all([
      listen("menu-open", () => handleOpenFile()),
      listen("menu-reload", () => {
        if (filePath) openFile(filePath);
      }),
      listen("menu-check-update", async () => {
        try {
          const update = await check();
          if (update?.available) {
            setUpdateAvailable(true);
            setUpdateToast(useI18n.getState().t.updateToastAvailable);
          } else {
            setUpdateToast(useI18n.getState().t.updateToastLatest);
          }
        } catch {
          setUpdateToast(useI18n.getState().t.updateToastError);
        }
        setTimeout(() => setUpdateToast(null), 4000);
      }),
      listen("menu-export", () => {
        if (!useJsonStore.getState().filePath) {
          setUpdateToast(useI18n.getState().t.exportNoFile);
          setTimeout(() => setUpdateToast(null), 3000);
        } else {
          setShowExport(true);
        }
      })
    ]).then((fns) => unlisteners.push(...fns));
    return () => unlisteners.forEach((fn) => fn());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath]);

  // Incolla JSON dalla clipboard
  useEffect(() => {
    const handler = async (e: ClipboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      const text = e.clipboardData?.getData("text/plain")?.trim();
      if (!text) return;
      if (!text.startsWith("{") && !text.startsWith("[")) return;
      try {
        await openFromString(text);
        setPasteError(null);
      } catch {
        setPasteError(useI18n.getState().t.pasteError);
        setTimeout(() => setPasteError(null), 3000);
      }
    };
    window.addEventListener("paste", handler);
    return () => window.removeEventListener("paste", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openFromString]);

  // Drag & drop
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebviewWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over")
          setIsDragging(true);
        else if (event.payload.type === "leave") setIsDragging(false);
        else if (event.payload.type === "drop") {
          setIsDragging(false);
          const paths = (event.payload as { type: "drop"; paths: string[] })
            .paths;
          const jsonFile = paths.find((p) => p.toLowerCase().endsWith(".json"));
          if (jsonFile) openFile(jsonFile);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, [openFile]);

  return (
    <div className="h-screen flex flex-col bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100 relative">
      {/* Progress bar */}
      {loading && (
        <div className="absolute inset-x-0 top-0 z-50 h-0.5 bg-gray-200 dark:bg-gray-700">
          {parseProgress !== null ? (
            <div
              className="h-full bg-blue-500 transition-all duration-150"
              style={{ width: `${parseProgress}%` }}
            />
          ) : (
            <div className="h-full w-full bg-blue-500 animate-pulse" />
          )}
        </div>
      )}

      {/* Overlay drag & drop */}
      {isDragging && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-blue-100/60 dark:bg-blue-900/60 border-4 border-dashed border-blue-500 dark:border-blue-400 pointer-events-none">
          <div className="text-blue-800 dark:text-blue-200 text-lg font-medium">
            Rilascia il file JSON
          </div>
        </div>
      )}

      <Toolbar
        onOpenFile={handleOpenFile}
        onUpdate={handleUpdate}
        updateAvailable={updateAvailable}
        updating={updating}
        darkMode={darkMode}
        onDarkModeToggle={() => setDarkMode((v) => !v)}
      />

      {/* Contenuto principale — 3 colonne */}
      <div className="flex flex-1 overflow-hidden">
        <div
          style={{ width: leftWidth }}
          className="flex-shrink-0 overflow-hidden"
        >
          <SearchPanel />
        </div>

        <ResizeHandle direction="horizontal" onResize={handleLeftResize} />

        <TreePanel />

        <ResizeHandle direction="horizontal" onResize={handleRightResize} />

        {/* Colonna destra: Properties */}
        <div
          style={{ width: rightWidth }}
          className="flex flex-col border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex-shrink-0"
        >
          <div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 text-xs font-medium text-gray-500 dark:text-gray-400 flex-shrink-0">
            <PropertiesHeader />
          </div>
          <div className="flex-1 overflow-hidden">
            <PropertiesPanel />
          </div>
        </div>
      </div>

      <StatusBar />

      <ContextMenu />

      {pasteError && (
        <div className="fixed bottom-8 left-1/2 -translate-x-1/2 bg-red-600 text-white text-xs px-4 py-2 rounded shadow-lg z-50">
          {pasteError}
        </div>
      )}

      {updateToast && (
        <div className="fixed bottom-8 left-1/2 -translate-x-1/2 bg-gray-800 dark:bg-gray-700 text-white text-xs px-4 py-2 rounded shadow-lg z-50">
          {updateToast}
        </div>
      )}

      {showExport && (
        <ExportModal
          filePath={filePath}
          onClose={() => setShowExport(false)}
          onError={(msg) => {
            setUpdateToast(msg);
            setTimeout(() => setUpdateToast(null), 4000);
          }}
        />
      )}
    </div>
  );
}

function PropertiesHeader() {
  const { t } = useI18n();
  return <>{t.propertiesHeader}</>;
}

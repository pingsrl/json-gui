import { useRef, useState, useEffect, type FC } from "react";
import { FolderOpen, Clock, Sun, Moon, Download } from "lucide-react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

interface ToolbarProps {
  onOpenFile: () => void;
  onUpdate: () => void;
  updateAvailable: boolean;
  updating: boolean;
  darkMode: boolean;
  onDarkModeToggle: () => void;
}

export const Toolbar: FC<ToolbarProps> = ({
  onOpenFile,
  onUpdate,
  updateAvailable,
  updating,
  darkMode,
  onDarkModeToggle
}) => {
  const { recentFiles, filePath, openFile } = useJsonStore();
  const { t, lang, setLang } = useI18n();
  const [recentOpen, setRecentOpen] = useState(false);
  const recentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!recentOpen) return;
    const handler = (e: MouseEvent) => {
      if (recentRef.current && !recentRef.current.contains(e.target as Node))
        setRecentOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [recentOpen]);

  return (
    <div className="flex items-center gap-2 px-3 py-2 bg-white dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
      <button
        onClick={onOpenFile}
        className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 hover:bg-blue-500 rounded text-sm font-medium text-white transition-colors"
      >
        <FolderOpen size={14} />
        {t.openFile}
      </button>

      {recentFiles.length > 0 && (
        <div className="relative" ref={recentRef}>
          <button
            onClick={() => setRecentOpen((v) => !v)}
            className="flex items-center gap-1.5 px-2 py-1.5 bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 rounded text-sm transition-colors"
            title={t.recentTitle}
          >
            <Clock size={14} />
            {t.recent}
          </button>
          {recentOpen && (
            <div className="absolute left-0 top-full mt-1 z-40 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded shadow-lg py-1 min-w-[320px]">
              {recentFiles.map((rf) => {
                const name = rf.split("/").pop() ?? rf;
                const dir = rf
                  .slice(0, rf.length - name.length)
                  .replace(/\/$/, "");
                return (
                  <button
                    key={rf}
                    className="w-full text-left px-3 py-2 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
                    title={rf}
                    onClick={() => {
                      setRecentOpen(false);
                      openFile(rf);
                    }}
                  >
                    <div className="text-sm text-gray-800 dark:text-gray-100 truncate">
                      {name}
                    </div>
                    <div className="text-xs text-gray-400 dark:text-gray-500 truncate font-mono">
                      {dir}
                    </div>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}

      <span className="text-gray-400 dark:text-gray-500 text-sm truncate flex-1">
        {filePath ?? t.noFileOpen}
      </span>

      {updateAvailable && (
        <button
          onClick={onUpdate}
          disabled={updating}
          className="flex items-center gap-1.5 px-2 py-1.5 bg-emerald-600 hover:bg-emerald-500 disabled:opacity-60 rounded text-sm text-white transition-colors"
          title={t.updateAvailable}
        >
          <Download size={14} />
          {updating ? t.updating : t.update}
        </button>
      )}

      <button
        onClick={onDarkModeToggle}
        className="p-1.5 rounded text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
        title={darkMode ? t.lightTheme : t.darkTheme}
      >
        {darkMode ? <Sun size={16} /> : <Moon size={16} />}
      </button>

      <div className="flex rounded overflow-hidden border border-gray-200 dark:border-gray-600 text-xs">
        {(["en", "it", "zh"] as const).map((l) => (
          <button
            key={l}
            onClick={() => setLang(l)}
            className={`px-2 py-1 transition-colors ${
              lang === l
                ? "bg-blue-600 text-white"
                : "text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700"
            }`}
          >
            {l === "zh" ? "中文" : l.toUpperCase()}
          </button>
        ))}
      </div>
    </div>
  );
};

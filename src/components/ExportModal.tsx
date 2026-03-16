import { useState, type FC } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { useI18n } from "../i18n";

interface Lang {
  id: string;
  label: string;
  ext: string;
  filterName: string;
  badge: string;
}

const LANGS: Lang[] = [
  {
    id: "typescript",
    label: "TypeScript",
    ext: "ts",
    filterName: "TypeScript",
    badge: "bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300"
  },
  {
    id: "zod",
    label: "Zod",
    ext: "ts",
    filterName: "TypeScript",
    badge:
      "bg-indigo-100 text-indigo-700 dark:bg-indigo-900/40 dark:text-indigo-300"
  },
  {
    id: "rust",
    label: "Rust",
    ext: "rs",
    filterName: "Rust source",
    badge:
      "bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-300"
  },
  {
    id: "go",
    label: "Go",
    ext: "go",
    filterName: "Go source",
    badge: "bg-cyan-100 text-cyan-700 dark:bg-cyan-900/40 dark:text-cyan-300"
  },
  {
    id: "python",
    label: "Python",
    ext: "py",
    filterName: "Python",
    badge:
      "bg-yellow-100 text-yellow-700 dark:bg-yellow-900/40 dark:text-yellow-300"
  },
  {
    id: "json-schema",
    label: "JSON Schema",
    ext: "json",
    filterName: "JSON",
    badge:
      "bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300"
  }
];

interface Props {
  onClose: () => void;
  filePath: string | null;
  onError: (msg: string) => void;
}

export const ExportModal: FC<Props> = ({ onClose, filePath, onError }) => {
  const { t } = useI18n();
  const [loading, setLoading] = useState<string | null>(null);

  const suggestedName = (ext: string) => {
    if (!filePath) return `types.${ext}`;
    const base = filePath.replace(/\\/g, "/").split("/").pop() ?? "types";
    return base.replace(/\.json$/i, `.${ext}`);
  };

  const handleSelect = async (lang: Lang) => {
    if (loading) return;
    setLoading(lang.id);
    try {
      const content = await invoke<string>("export_types", { lang: lang.id });
      const dest = await save({
        defaultPath: suggestedName(lang.ext),
        filters: [{ name: lang.filterName, extensions: [lang.ext] }]
      });
      if (dest) {
        await writeTextFile(dest, content);
      }
      onClose();
    } catch (err) {
      onError(String(err));
      onClose();
    } finally {
      setLoading(null);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="bg-white dark:bg-gray-900 rounded-xl shadow-2xl w-[400px] max-w-[90vw] overflow-hidden">
        {/* Header */}
        <div className="px-5 py-4 border-b border-gray-200 dark:border-gray-700">
          <h2 className="text-sm font-semibold text-gray-900 dark:text-gray-100">
            {t.exportTitle}
          </h2>
          <p className="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
            {t.exportSubtitle}
          </p>
        </div>

        {/* Language grid */}
        <div className="p-4 grid grid-cols-2 gap-2">
          {LANGS.map((lang) => (
            <button
              key={lang.id}
              onClick={() => handleSelect(lang)}
              disabled={!!loading}
              className={`flex items-center gap-2.5 px-3 py-2.5 rounded-lg border text-left transition-colors
                border-gray-200 dark:border-gray-700
                hover:bg-gray-50 dark:hover:bg-gray-800
                disabled:opacity-50 disabled:cursor-not-allowed
                ${loading === lang.id ? "bg-gray-50 dark:bg-gray-800" : ""}`}
            >
              <span
                className={`text-xs font-medium px-1.5 py-0.5 rounded flex-shrink-0 ${lang.badge}`}
              >
                .{lang.ext}
              </span>
              <span className="text-xs font-mono text-gray-700 dark:text-gray-300 truncate">
                {loading === lang.id ? "…" : lang.label}
              </span>
            </button>
          ))}
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-gray-200 dark:border-gray-700 flex justify-end">
          <button
            onClick={onClose}
            className="text-xs px-3 py-1.5 rounded text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700"
          >
            Annulla
          </button>
        </div>
      </div>
    </div>
  );
};

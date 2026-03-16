import { create } from "zustand";

export type Lang = "en" | "it" | "zh";

const translations = {
  en: {
    openFile: "Open file",
    recent: "Recent",
    recentTitle: "Recent files",
    noFileOpen: "No file open",
    updateAvailable: "Update available",
    updating: "Updating...",
    update: "Update",
    lightTheme: "Light theme",
    darkTheme: "Dark theme",
    searchPlaceholder: "Search... (Cmd+F)",
    searchBoth: "both",
    searchKeys: "keys",
    searchValues: "values",
    caseSensitive: "case sensitive",
    regex: "regex",
    exactMatch: "exact",
    searching: "Searching...",
    results: (n: number) => `${n} results`,
    limitReached: "(limit reached)",
    noResults: "No results found",
    searchHint: (n: string) => `Type to search among ${n} nodes`,
    expandAll: "Expand all",
    collapseAll: "Collapse all",
    openJsonFile: "Open a JSON file to start",
    anySize: "Supports files of any size",
    propertiesHeader: "Properties",
    nodes: (n: string) => `Nodes: ${n}`,
    size: (s: string) => `Size: ${s}`,
    updateToastAvailable: "Update available! Use the toolbar button.",
    updateToastLatest: "You're already on the latest version.",
    updateToastError: "Unable to check for updates.",
    pasteError: "Pasted text is not valid JSON",
    parentObject: "Parent object",
    siblings: "Siblings",
    noSiblings: "No siblings",
    selectNode: "Select a node",
    path: "Path",
    value: "Value",
    keys: "Keys",
    elements: "Elements",
    propertiesPlaceholder: "Properties",
    copyPath: "Copy path",
    copyValue: "Copy value",
    copyRaw: "Copy raw JSON"
  },
  it: {
    openFile: "Apri file",
    recent: "Recenti",
    recentTitle: "File recenti",
    noFileOpen: "Nessun file aperto",
    updateAvailable: "Aggiornamento disponibile",
    updating: "Aggiornamento...",
    update: "Aggiorna",
    lightTheme: "Tema chiaro",
    darkTheme: "Tema scuro",
    searchPlaceholder: "Cerca... (Cmd+F)",
    searchBoth: "entrambi",
    searchKeys: "chiavi",
    searchValues: "valori",
    caseSensitive: "case sensitive",
    regex: "regex",
    exactMatch: "esatta",
    searching: "Ricerca in corso...",
    results: (n: number) => `${n} risultati`,
    limitReached: "(limite raggiunto)",
    noResults: "Nessun risultato trovato",
    searchHint: (n: string) => `Digita per cercare tra ${n} nodi`,
    expandAll: "Apri tutto",
    collapseAll: "Chiudi tutto",
    openJsonFile: "Apri un file JSON per iniziare",
    anySize: "Supporta file di qualsiasi dimensione",
    propertiesHeader: "Proprietà",
    nodes: (n: string) => `Nodi: ${n}`,
    size: (s: string) => `Dimensione: ${s}`,
    updateToastAvailable:
      "Aggiornamento disponibile! Usa il pulsante in barra.",
    updateToastLatest: "Sei già all'ultima versione.",
    updateToastError: "Impossibile controllare gli aggiornamenti.",
    pasteError: "Il testo incollato non è un JSON valido",
    parentObject: "Oggetto padre",
    siblings: "Fratelli",
    noSiblings: "Nessun fratello",
    selectNode: "Seleziona un nodo",
    path: "Path",
    value: "Valore",
    keys: "Chiavi",
    elements: "Elementi",
    propertiesPlaceholder: "Proprietà",
    copyPath: "Copia path",
    copyValue: "Copia valore",
    copyRaw: "Copia raw JSON"
  },
  zh: {
    openFile: "打开文件",
    recent: "最近",
    recentTitle: "最近文件",
    noFileOpen: "未打开文件",
    updateAvailable: "有可用更新",
    updating: "更新中...",
    update: "更新",
    lightTheme: "浅色主题",
    darkTheme: "深色主题",
    searchPlaceholder: "搜索... (Cmd+F)",
    searchBoth: "全部",
    searchKeys: "键",
    searchValues: "值",
    caseSensitive: "区分大小写",
    regex: "正则",
    exactMatch: "精确",
    searching: "搜索中...",
    results: (n: number) => `${n} 个结果`,
    limitReached: "（已达上限）",
    noResults: "未找到结果",
    searchHint: (n: string) => `输入以搜索 ${n} 个节点`,
    expandAll: "全部展开",
    collapseAll: "全部折叠",
    openJsonFile: "打开 JSON 文件以开始",
    anySize: "支持任意大小的文件",
    propertiesHeader: "属性",
    nodes: (n: string) => `节点数: ${n}`,
    size: (s: string) => `大小: ${s}`,
    updateToastAvailable: "有可用更新！使用工具栏中的按钮。",
    updateToastLatest: "您已是最新版本。",
    updateToastError: "无法检查更新。",
    pasteError: "粘贴的文本不是有效的 JSON",
    parentObject: "父对象",
    siblings: "兄弟节点",
    noSiblings: "无兄弟节点",
    selectNode: "选择一个节点",
    path: "路径",
    value: "值",
    keys: "键",
    elements: "元素",
    propertiesPlaceholder: "属性",
    copyPath: "复制路径",
    copyValue: "复制值",
    copyRaw: "复制原始 JSON"
  }
} as const;

export type Translations = typeof translations.en;

function loadLang(): Lang {
  const saved = localStorage.getItem("lang");
  if (saved === "en" || saved === "it" || saved === "zh") return saved;
  const browser = navigator.language.slice(0, 2);
  if (browser === "it") return "it";
  if (browser === "zh") return "zh";
  return "en";
}

interface I18nStore {
  lang: Lang;
  t: Translations;
  setLang: (lang: Lang) => void;
}

export const useI18n = create<I18nStore>((set) => ({
  lang: loadLang(),
  t: translations[loadLang()],
  setLang: (lang: Lang) => {
    localStorage.setItem("lang", lang);
    set({ lang, t: translations[lang] });
  }
}));

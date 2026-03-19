import { create } from "zustand";

export type Lang = "en" | "it" | "zh";

export interface Translations {
  openFile: string;
  recent: string;
  recentTitle: string;
  noFileOpen: string;
  updateAvailable: string;
  updating: string;
  update: string;
  lightTheme: string;
  darkTheme: string;
  searchPlaceholder: string;
  searchModeText: string;
  searchModeObjects: string;
  searchScope: string;
  searchFilters: string;
  searchPath: string;
  searchPathPlaceholder: string;
  clearSearchScope: string;
  searchSort: string;
  searchSortLabel: string;
  searchSortRelevance: string;
  searchSortFileOrder: string;
  searchBoth: string;
  searchKeys: string;
  searchValues: string;
  caseSensitive: string;
  caseSensitiveKey: string;
  caseSensitiveValue: string;
  regex: string;
  exactMatch: string;
  searching: string;
  apply: string;
  applyAll: string;
  resetFilters: string;
  applied: string;
  results: (n: number) => string;
  limitReached: string;
  noResults: string;
  searchHint: (n: string) => string;
  objectSearchHint: string;
  objectPathPlaceholder: string;
  objectValuePlaceholder: string;
  objectOperatorContains: string;
  objectOperatorEquals: string;
  objectOperatorRegex: string;
  objectOperatorExists: string;
  objectFilters: string;
  expandAll: string;
  collapseAll: string;
  openJsonFile: string;
  anySize: string;
  propertiesHeader: string;
  nodes: (n: string) => string;
  size: (s: string) => string;
  ram: (s: string) => string;
  cpu: (s: string) => string;
  lastOperation: string;
  noOperation: string;
  updateToastAvailable: string;
  updateToastLatest: string;
  updateToastError: string;
  pasteError: string;
  parentObject: string;
  siblings: string;
  noSiblings: string;
  selectNode: string;
  path: string;
  value: string;
  keys: string;
  elements: string;
  propertiesPlaceholder: string;
  copyKey: string;
  copyPath: string;
  copyValue: string;
  copyRaw: string;
  expandFromHere: string;
  openInNewWindow: string;
  searchInNode: string;
  searchInParentNode: string;
  exportTitle: string;
  exportSubtitle: string;
  exportNoFile: string;
  exportSaveError: string;
  regexError: (msg: string) => string;
}

const translations: Record<Lang, Translations> = {
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
    searchModeText: "Text",
    searchModeObjects: "Objects",
    searchScope: "Scope",
    searchFilters: "Filters",
    searchPath: "Path",
    searchPathPlaceholder: "$.users.0",
    clearSearchScope: "Clear path filter",
    searchSort: "Sort",
    searchSortLabel: "sort by",
    searchSortRelevance: "relevance",
    searchSortFileOrder: "position",
    searchBoth: "both",
    searchKeys: "keys",
    searchValues: "values",
    caseSensitive: "case sensitive",
    caseSensitiveKey: "key case sensitive",
    caseSensitiveValue: "value case sensitive",
    regex: "regex",
    exactMatch: "exact",
    searching: "Searching...",
    apply: "Apply",
    applyAll: "Apply all",
    resetFilters: "Reset",
    applied: "Applied",
    results: (n: number) => `${n} results`,
    limitReached: "(limit reached)",
    noResults: "No results found",
    searchHint: (n: string) => `Type to search among ${n} nodes`,
    objectSearchHint: "Add one or more property filters and apply them",
    objectPathPlaceholder: "key",
    objectValuePlaceholder: "value",
    objectOperatorContains: "contains",
    objectOperatorEquals: "equals",
    objectOperatorRegex: "regex",
    objectOperatorExists: "exists",
    objectFilters: "Object filters",
    expandAll: "Expand all",
    collapseAll: "Collapse all",
    openJsonFile: "Open a JSON file to start",
    anySize: "Supports files of any size",
    propertiesHeader: "Properties",
    nodes: (n: string) => `Nodes: ${n}`,
    size: (s: string) => `Size: ${s}`,
    ram: (s: string) => `RAM: ${s}`,
    cpu: (s: string) => `CPU: ${s}`,
    lastOperation: "Last op",
    noOperation: "No operations yet",
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
    copyKey: "Copy key",
    copyPath: "Copy path",
    copyValue: "Copy value",
    copyRaw: "Copy raw JSON",
    expandFromHere: "Expand from here",
    openInNewWindow: "Open in new window",
    searchInNode: "Search in this node",
    searchInParentNode: "Search in parent node",
    exportTitle: "Export type definition",
    exportSubtitle: "Select target language",
    exportNoFile: "No file is open",
    exportSaveError: "Could not save file",
    regexError: (msg: string) => {
      const m = msg.toLowerCase();
      if (m.includes("nothing to repeat")) return "Nothing to repeat";
      if (m.includes("unterminated group") || m.includes("missing )"))
        return "Unterminated group";
      if (m.includes("unmatched )") || m.includes("unmatched ')'"))
        return "Unmatched ')'";
      if (m.includes("invalid group")) return "Invalid group";
      if (m.includes("range out of order"))
        return "Range out of order in character class";
      if (m.includes("invalid escape")) return "Invalid escape sequence";
      if (m.includes("too many") || m.includes("too large"))
        return "Quantifier too large";
      return "Invalid regex";
    }
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
    searchModeText: "Testo",
    searchModeObjects: "Oggetti",
    searchScope: "Ambito",
    searchFilters: "Filtri",
    searchPath: "Path",
    searchPathPlaceholder: "$.users.0",
    clearSearchScope: "Rimuovi filtro path",
    searchSort: "Ordina",
    searchSortLabel: "ordina per",
    searchSortRelevance: "pertinenza",
    searchSortFileOrder: "posizione",
    searchBoth: "entrambi",
    searchKeys: "chiavi",
    searchValues: "valori",
    caseSensitive: "case sensitive",
    caseSensitiveKey: "chiave case sensitive",
    caseSensitiveValue: "valore case sensitive",
    regex: "regex",
    exactMatch: "esatta",
    searching: "Ricerca in corso...",
    apply: "Applica",
    applyAll: "Applica tutti",
    resetFilters: "Reset",
    applied: "Applicato",
    results: (n: number) => `${n} risultati`,
    limitReached: "(limite raggiunto)",
    noResults: "Nessun risultato trovato",
    searchHint: (n: string) => `Digita per cercare tra ${n} nodi`,
    objectSearchHint: "Aggiungi uno o più filtri proprietà e applicali",
    objectPathPlaceholder: "chiave",
    objectValuePlaceholder: "valore",
    objectOperatorContains: "contiene",
    objectOperatorEquals: "uguale",
    objectOperatorRegex: "regex",
    objectOperatorExists: "esiste",
    objectFilters: "Filtri oggetto",
    expandAll: "Apri tutto",
    collapseAll: "Chiudi tutto",
    openJsonFile: "Apri un file JSON per iniziare",
    anySize: "Supporta file di qualsiasi dimensione",
    propertiesHeader: "Proprietà",
    nodes: (n: string) => `Nodi: ${n}`,
    size: (s: string) => `Dimensione: ${s}`,
    ram: (s: string) => `RAM: ${s}`,
    cpu: (s: string) => `CPU: ${s}`,
    lastOperation: "Ultima op",
    noOperation: "Nessuna operazione",
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
    copyKey: "Copia chiave",
    copyPath: "Copia path",
    copyValue: "Copia valore",
    copyRaw: "Copia raw JSON",
    expandFromHere: "Espandi da qui",
    openInNewWindow: "Apri in nuova finestra",
    searchInNode: "Cerca in questo nodo",
    searchInParentNode: "Cerca nel nodo padre",
    exportTitle: "Esporta definizione tipo",
    exportSubtitle: "Seleziona il linguaggio",
    exportNoFile: "Nessun file aperto",
    exportSaveError: "Impossibile salvare il file",
    regexError: (msg: string) => {
      const m = msg.toLowerCase();
      if (m.includes("nothing to repeat")) return "Niente da ripetere";
      if (m.includes("unterminated group") || m.includes("missing )"))
        return "Gruppo non chiuso";
      if (m.includes("unmatched )") || m.includes("unmatched ')'"))
        return "')' senza corrispondenza";
      if (m.includes("invalid group")) return "Gruppo non valido";
      if (m.includes("range out of order"))
        return "Intervallo non valido nella classe di caratteri";
      if (m.includes("invalid escape")) return "Sequenza di escape non valida";
      if (m.includes("too many") || m.includes("too large"))
        return "Quantificatore troppo grande";
      return "Espressione regolare non valida";
    }
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
    searchModeText: "文本",
    searchModeObjects: "对象",
    searchScope: "范围",
    searchFilters: "筛选",
    searchPath: "路径",
    searchPathPlaceholder: "$.users.0",
    clearSearchScope: "清除路径过滤",
    searchSort: "排序",
    searchSortLabel: "排序方式",
    searchSortRelevance: "相关性",
    searchSortFileOrder: "位置",
    searchBoth: "全部",
    searchKeys: "键",
    searchValues: "值",
    caseSensitive: "区分大小写",
    caseSensitiveKey: "键区分大小写",
    caseSensitiveValue: "值区分大小写",
    regex: "正则",
    exactMatch: "精确",
    searching: "搜索中...",
    apply: "应用",
    applyAll: "全部应用",
    resetFilters: "重置",
    applied: "已应用",
    results: (n: number) => `${n} 个结果`,
    limitReached: "（已达上限）",
    noResults: "未找到结果",
    searchHint: (n: string) => `输入以搜索 ${n} 个节点`,
    objectSearchHint: "添加一个或多个属性过滤器并应用",
    objectPathPlaceholder: "键",
    objectValuePlaceholder: "值",
    objectOperatorContains: "包含",
    objectOperatorEquals: "等于",
    objectOperatorRegex: "正则",
    objectOperatorExists: "存在",
    objectFilters: "对象过滤器",
    expandAll: "全部展开",
    collapseAll: "全部折叠",
    openJsonFile: "打开 JSON 文件以开始",
    anySize: "支持任意大小的文件",
    propertiesHeader: "属性",
    nodes: (n: string) => `节点数: ${n}`,
    size: (s: string) => `大小: ${s}`,
    ram: (s: string) => `内存: ${s}`,
    cpu: (s: string) => `CPU: ${s}`,
    lastOperation: "最近操作",
    noOperation: "暂无操作",
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
    copyKey: "复制键名",
    copyPath: "复制路径",
    copyValue: "复制值",
    copyRaw: "复制原始 JSON",
    expandFromHere: "从此处展开",
    openInNewWindow: "在新窗口中打开",
    searchInNode: "在此节点中搜索",
    searchInParentNode: "在父节点中搜索",
    exportTitle: "导出类型定义",
    exportSubtitle: "选择目标语言",
    exportNoFile: "未打开文件",
    exportSaveError: "无法保存文件",
    regexError: (msg: string) => {
      const m = msg.toLowerCase();
      if (m.includes("nothing to repeat")) return "没有可重复的内容";
      if (m.includes("unterminated group") || m.includes("missing )"))
        return "未闭合的分组";
      if (m.includes("unmatched )") || m.includes("unmatched ')'"))
        return "多余的 ')'";
      if (m.includes("invalid group")) return "无效的分组";
      if (m.includes("range out of order")) return "字符类中范围顺序错误";
      if (m.includes("invalid escape")) return "无效的转义序列";
      if (m.includes("too many") || m.includes("too large")) return "量词过大";
      return "无效的正则表达式";
    }
  }
};

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

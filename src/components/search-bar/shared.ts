import type { Translations } from "../../i18n";
import type {
  ObjectSearchFilter,
  SearchMode,
  SearchSortMode
} from "../../store";

type TranslationKey = keyof Translations;

export const TEXT_TARGETS = [
  { value: "both", labelKey: "searchBoth" },
  { value: "keys", labelKey: "searchKeys" },
  { value: "values", labelKey: "searchValues" }
] as const satisfies ReadonlyArray<{
  value: string;
  labelKey: TranslationKey;
}>;

export type TextSearchTarget = (typeof TEXT_TARGETS)[number]["value"];

export type FilterState = {
  caseSensitive: boolean;
  useRegex: boolean;
  exactMatch: boolean;
};

export type FilterSetters = {
  setCaseSensitive: (value: boolean) => void;
  setUseRegex: (value: boolean) => void;
  setExactMatch: (value: boolean) => void;
};

export const TEXT_MODIFIERS = [
  {
    key: "caseSensitive" as const,
    getChecked: (state: FilterState) => state.caseSensitive,
    onChange: (checked: boolean, setters: FilterSetters) => {
      if (checked) setters.setUseRegex(false);
      setters.setCaseSensitive(checked);
    },
    labelKey: "caseSensitive" as const
  },
  {
    key: "exactMatch" as const,
    getChecked: (state: FilterState) => state.exactMatch,
    onChange: (checked: boolean, setters: FilterSetters) => {
      if (checked) setters.setUseRegex(false);
      setters.setExactMatch(checked);
    },
    labelKey: "exactMatch" as const
  }
] as const;

export const TEXT_REGEX_FILTER = {
  key: "regex" as const,
  getChecked: (state: FilterState) => state.useRegex,
  onChange: (checked: boolean, setters: FilterSetters) => {
    if (checked) {
      setters.setCaseSensitive(false);
      setters.setExactMatch(false);
    }
    setters.setUseRegex(checked);
  },
  labelKey: "regex" as const
};

export const OBJECT_OPERATORS = [
  { value: "contains", labelKey: "objectOperatorContains" },
  { value: "equals", labelKey: "objectOperatorEquals" },
  { value: "regex", labelKey: "objectOperatorRegex" },
  { value: "exists", labelKey: "objectOperatorExists" }
] as const satisfies ReadonlyArray<{
  value: ObjectSearchFilter["operator"];
  labelKey: TranslationKey;
}>;

export const MODE_OPTIONS = [
  { value: "text", labelKey: "searchModeText" },
  { value: "object", labelKey: "searchModeObjects" }
] as const satisfies ReadonlyArray<{
  value: SearchMode;
  labelKey: TranslationKey;
}>;

export const PATH_SUGGESTION_LIMIT = 12;
const SEARCH_FILTERS_STORAGE_PREFIX = "searchFilters:";
export const MAX_SUGGESTION_CACHE = 50;

export interface ObjectFilterRow {
  id: number;
  enabled: boolean;
  path: string;
  operator: ObjectSearchFilter["operator"];
  value: string;
  regexCaseInsensitive: boolean;
  regexMultiline: boolean;
  regexDotAll: boolean;
}

export interface PersistedSearchFilters {
  version: 1;
  searchMode: SearchMode;
  searchQuery: string;
  searchTarget: TextSearchTarget;
  caseSensitive: boolean;
  useRegex: boolean;
  exactMatch: boolean;
  searchScopePath: string;
  searchSort: SearchSortMode;
  objectKeyCaseSensitive: boolean;
  objectValueCaseSensitive: boolean;
  objectRows: ObjectFilterRow[];
}

export interface AutocompleteDropdownStyle {
  top: number;
  left: number;
  width: number;
}

export function buildObjectFilterRow(id: number): ObjectFilterRow {
  return {
    id,
    enabled: true,
    path: "",
    operator: "contains",
    value: "",
    regexCaseInsensitive: false,
    regexMultiline: false,
    regexDotAll: false
  };
}

export function getObjectFilterFingerprint(row: ObjectFilterRow): string {
  return JSON.stringify({
    enabled: row.enabled,
    path: row.path.trim(),
    operator: row.operator,
    value: row.value.trim(),
    regexCaseInsensitive: row.regexCaseInsensitive,
    regexMultiline: row.regexMultiline,
    regexDotAll: row.regexDotAll
  });
}

export function toObjectSearchFilter(
  row: ObjectFilterRow
): ObjectSearchFilter | null {
  const path = row.path.trim();
  if (!path) return null;
  if (row.operator !== "exists" && !row.value.trim()) return null;

  return {
    path,
    operator: row.operator,
    value: row.operator === "exists" ? undefined : row.value.trim(),
    regexCaseInsensitive: row.regexCaseInsensitive,
    regexMultiline: row.regexMultiline,
    regexDotAll: row.regexDotAll
  };
}

export function getInlineAutocompleteSuggestion(
  inputValue: string,
  suggestions: string[],
  index: number
): string | null {
  const suggestion = suggestions[index];
  if (!suggestion) return null;
  if (!inputValue) return suggestion;

  return suggestion.toLowerCase().startsWith(inputValue.toLowerCase())
    ? suggestion
    : null;
}

export function getSearchFiltersStorageKey(filePath: string): string {
  return `${SEARCH_FILTERS_STORAGE_PREFIX}${filePath}`;
}

export function isPersistentFilePath(
  filePath: string | null
): filePath is string {
  return !!filePath && filePath !== "(incollato)";
}

function sanitizePersistedRows(rows: unknown): ObjectFilterRow[] {
  if (!Array.isArray(rows)) {
    return [buildObjectFilterRow(1)];
  }

  const sanitized = rows.flatMap((row, index) => {
    if (!row || typeof row !== "object") {
      return [];
    }

    const candidate = row as Partial<ObjectFilterRow>;
    const operator = candidate.operator;
    if (
      operator !== "contains" &&
      operator !== "equals" &&
      operator !== "regex" &&
      operator !== "exists"
    ) {
      return [];
    }

    return [
      {
        id:
          typeof candidate.id === "number" && Number.isFinite(candidate.id)
            ? candidate.id
            : index + 1,
        enabled:
          typeof candidate.enabled === "boolean" ? candidate.enabled : true,
        path: typeof candidate.path === "string" ? candidate.path : "",
        operator,
        value: typeof candidate.value === "string" ? candidate.value : "",
        regexCaseInsensitive:
          typeof candidate.regexCaseInsensitive === "boolean"
            ? candidate.regexCaseInsensitive
            : false,
        regexMultiline:
          typeof candidate.regexMultiline === "boolean"
            ? candidate.regexMultiline
            : false,
        regexDotAll:
          typeof candidate.regexDotAll === "boolean"
            ? candidate.regexDotAll
            : false
      }
    ];
  });

  return sanitized.length > 0 ? sanitized : [buildObjectFilterRow(1)];
}

export function loadPersistedSearchFilters(
  filePath: string
): PersistedSearchFilters | null {
  try {
    const raw = localStorage.getItem(getSearchFiltersStorageKey(filePath));
    if (!raw) return null;

    const parsed = JSON.parse(raw) as Partial<PersistedSearchFilters>;
    const searchMode = parsed.searchMode === "object" ? "object" : "text";
    const searchSort = parsed.searchSort === "file" ? "file" : "relevance";

    return {
      version: 1,
      searchMode,
      searchQuery:
        typeof parsed.searchQuery === "string" ? parsed.searchQuery : "",
      searchTarget:
        parsed.searchTarget === "keys" ||
        parsed.searchTarget === "values" ||
        parsed.searchTarget === "both"
          ? parsed.searchTarget
          : "both",
      caseSensitive: Boolean(parsed.caseSensitive),
      useRegex: Boolean(parsed.useRegex),
      exactMatch: Boolean(parsed.exactMatch),
      searchScopePath:
        typeof parsed.searchScopePath === "string"
          ? parsed.searchScopePath
          : "",
      searchSort,
      objectKeyCaseSensitive: Boolean(parsed.objectKeyCaseSensitive),
      objectValueCaseSensitive: Boolean(parsed.objectValueCaseSensitive),
      objectRows: sanitizePersistedRows(parsed.objectRows)
    };
  } catch {
    return null;
  }
}

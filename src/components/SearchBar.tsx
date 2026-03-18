import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FC,
  type ChangeEvent,
  type KeyboardEvent
} from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Check, ChevronDown, Minus, Plus, Search, X } from "lucide-react";
import {
  type ObjectSearchFilter,
  type SearchMode,
  type SearchSortMode,
  useJsonStore
} from "../store";
import { useI18n } from "../i18n";

const TEXT_TARGETS = [
  { value: "both", labelKey: "searchBoth" },
  { value: "keys", labelKey: "searchKeys" },
  { value: "values", labelKey: "searchValues" }
] as const;


const TEXT_FILTERS = [
  {
    key: "caseSensitive",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.caseSensitive,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setCaseSensitive(checked),
    labelKey: "caseSensitive"
  },
  {
    key: "regex",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.useRegex,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setUseRegex(checked),
    labelKey: "regex"
  },
  {
    key: "exactMatch",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.exactMatch,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setExactMatch(checked),
    labelKey: "exactMatch"
  }
] as const;

const OBJECT_OPERATORS = [
  { value: "contains", labelKey: "objectOperatorContains" },
  { value: "equals", labelKey: "objectOperatorEquals" },
  { value: "regex", labelKey: "objectOperatorRegex" },
  { value: "exists", labelKey: "objectOperatorExists" }
] as const;

const MODE_OPTIONS = [
  { value: "text", labelKey: "searchModeText" },
  { value: "object", labelKey: "searchModeObjects" }
] as const;

const PATH_SUGGESTION_LIMIT = 12;
const SEARCH_FILTERS_STORAGE_PREFIX = "searchFilters:";
const MAX_SUGGESTION_CACHE = 50;

interface ObjectFilterRow {
  id: number;
  enabled: boolean;
  path: string;
  operator: ObjectSearchFilter["operator"];
  value: string;
}

interface PersistedSearchFilters {
  version: 1;
  searchMode: SearchMode;
  searchQuery: string;
  searchTarget: string;
  caseSensitive: boolean;
  useRegex: boolean;
  exactMatch: boolean;
  searchScopePath: string;
  searchSort: SearchSortMode;
  objectKeyCaseSensitive: boolean;
  objectValueCaseSensitive: boolean;
  objectRows: ObjectFilterRow[];
}

function buildObjectFilterRow(id: number): ObjectFilterRow {
  return {
    id,
    enabled: true,
    path: "",
    operator: "contains",
    value: ""
  };
}

function getObjectFilterFingerprint(row: ObjectFilterRow): string {
  return JSON.stringify({
    enabled: row.enabled,
    path: row.path.trim(),
    operator: row.operator,
    value: row.value.trim()
  });
}

function toObjectSearchFilter(
  row: ObjectFilterRow
): ObjectSearchFilter | null {
  const path = row.path.trim();
  if (!path) return null;
  if (row.operator !== "exists" && !row.value.trim()) return null;
  return {
    path,
    operator: row.operator,
    value: row.operator === "exists" ? undefined : row.value.trim()
  };
}

function getInlineAutocompleteSuggestion(
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

function getSearchFiltersStorageKey(filePath: string): string {
  return `${SEARCH_FILTERS_STORAGE_PREFIX}${filePath}`;
}

function isPersistentFilePath(filePath: string | null): filePath is string {
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
        value: typeof candidate.value === "string" ? candidate.value : ""
      }
    ];
  });

  return sanitized.length > 0 ? sanitized : [buildObjectFilterRow(1)];
}

function loadPersistedSearchFilters(
  filePath: string
): PersistedSearchFilters | null {
  try {
    const raw = localStorage.getItem(getSearchFiltersStorageKey(filePath));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PersistedSearchFilters>;
    const searchMode =
      parsed.searchMode === "object" ? "object" : "text";
    const searchSort =
      parsed.searchSort === "file" ? "file" : "relevance";
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
        typeof parsed.searchScopePath === "string" ? parsed.searchScopePath : "",
      searchSort,
      objectKeyCaseSensitive: Boolean(parsed.objectKeyCaseSensitive),
      objectValueCaseSensitive: Boolean(parsed.objectValueCaseSensitive),
      objectRows: sanitizePersistedRows(parsed.objectRows)
    };
  } catch {
    return null;
  }
}

export const SearchBar: FC = () => {
  const {
    filePath,
    nodeCount,
    searchMode,
    activeSearchMode,
    searchScopePath,
    searchSort,
    searching,
    setSearchMode,
    setSearchScopePath,
    setSearchSort,
    search,
    searchObjects,
    clearSearch
  } = useJsonStore();
  const { t } = useI18n();

  const [searchQuery, setSearchQuery] = useState("");
  const [searchTarget, setSearchTarget] = useState("both");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [exactMatch, setExactMatch] = useState(false);
  const [objectKeyCaseSensitive, setObjectKeyCaseSensitive] = useState(false);
  const [objectValueCaseSensitive, setObjectValueCaseSensitive] =
    useState(false);
  const [objectRows, setObjectRows] = useState<ObjectFilterRow[]>([
    buildObjectFilterRow(1)
  ]);
  const [appliedFingerprints, setAppliedFingerprints] = useState<
    Map<number, string>
  >(new Map());
  const [appliedObjectKeyCaseSensitive, setAppliedObjectKeyCaseSensitive] = useState<
    boolean | null
  >(null);
  const [appliedObjectValueCaseSensitive, setAppliedObjectValueCaseSensitive] =
    useState<
    boolean | null
  >(null);
  const [appliedScopePath, setAppliedScopePath] = useState<string>("");
  const [autocompleteRowId, setAutocompleteRowId] = useState<number | null>(null);
  const [autocompleteSuggestions, setAutocompleteSuggestions] = useState<
    string[]
  >([]);
  const [autocompleteIndex, setAutocompleteIndex] = useState(0);
  const [autocompleteDropdownStyle, setAutocompleteDropdownStyle] = useState<{
    top: number;
    left: number;
    width: number;
  } | null>(null);
  const [suppressInlineAutocompleteRowId, setSuppressInlineAutocompleteRowId] =
    useState<number | null>(null);

  const searchTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const suggestionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const suggestionCache = useRef(new Map<string, string[]>());
  const pathInputRefs = useRef(new Map<number, HTMLInputElement>());
  const latestAutocompleteRequest = useRef<{ rowId: number; prefix: string } | null>(
    null
  );
  const restoredFilePath = useRef<string | null>(null);
  const nextRowId = useRef(2);

  const filterState = { caseSensitive, useRegex, exactMatch };
  const filterSetters = { setCaseSensitive, setUseRegex, setExactMatch };
  const primaryObjectRowId = objectRows[0]?.id ?? null;

  const scheduleSearch = useCallback(
    (query: string, path: string) => {
      clearTimeout(searchTimer.current);
      if (!query.trim()) {
        clearSearch();
        return;
      }
      searchTimer.current = setTimeout(() => {
        search(query, searchTarget, caseSensitive, useRegex, exactMatch, path);
      }, 150);
    },
    [
      caseSensitive,
      clearSearch,
      exactMatch,
      search,
      searchTarget,
      useRegex
    ]
  );

  const requestAutocomplete = useCallback(
    (rowId: number, value: string) => {
      clearTimeout(suggestionTimer.current);
      const trimmed = value.trim();
      latestAutocompleteRequest.current = { rowId, prefix: trimmed };

      const cached = suggestionCache.current.get(trimmed);
      if (cached) {
        setAutocompleteRowId(rowId);
        setAutocompleteSuggestions(cached);
        setAutocompleteIndex(0);
        return;
      }

      suggestionTimer.current = setTimeout(async () => {
        try {
          const suggestions = await invoke<string[]>("suggest_property_paths", {
            prefix: trimmed,
            limit: PATH_SUGGESTION_LIMIT
          });
          const latest = latestAutocompleteRequest.current;
          if (!latest || latest.rowId !== rowId || latest.prefix !== trimmed) {
            return;
          }
          // LRU semplice: rimuove la entry più vecchia se si supera il limite
          if (suggestionCache.current.size >= MAX_SUGGESTION_CACHE) {
            const firstKey = suggestionCache.current.keys().next().value;
            if (firstKey !== undefined) suggestionCache.current.delete(firstKey);
          }
          suggestionCache.current.set(trimmed, suggestions);
          setAutocompleteRowId(rowId);
          setAutocompleteSuggestions(suggestions);
          setAutocompleteIndex(0);
        } catch (err) {
          console.error("suggest_property_paths error:", err);
          setAutocompleteRowId(null);
          setAutocompleteSuggestions([]);
          setAutocompleteIndex(0);
        }
      }, 80);
    },
    []
  );

  const updateAutocompleteDropdownPosition = useCallback((rowId: number) => {
    const input = pathInputRefs.current.get(rowId);
    if (!input) {
      setAutocompleteDropdownStyle(null);
      return;
    }
    const rect = input.getBoundingClientRect();
    setAutocompleteDropdownStyle({
      top: rect.bottom + window.scrollY + 4,
      left: rect.left + window.scrollX,
      width: rect.width
    });
  }, []);

  const updateObjectRow = useCallback(
    (rowId: number, updater: (row: ObjectFilterRow) => ObjectFilterRow) => {
      setObjectRows((rows) =>
        rows.map((row) => (row.id === rowId ? updater(row) : row))
      );
    },
    []
  );

  const handleSearchQueryChange = useCallback(
    (query: string) => {
      setSearchQuery(query);
      scheduleSearch(query, searchScopePath);
    },
    [scheduleSearch, searchScopePath]
  );

  const handleClearTextSearch = () => {
    clearTimeout(searchTimer.current);
    setSearchQuery("");
    clearSearch();
  };

  const handleClearScope = () => {
    setSearchScopePath("");
  };

  const submitTextSearch = () => {
    const query = searchQuery.trim();
    if (!query) {
      clearSearch();
      return;
    }
    clearTimeout(searchTimer.current);
    void search(
      query,
      searchTarget,
      caseSensitive,
      useRegex,
      exactMatch,
      searchScopePath
    );
  };

  const handleInsertRowAfter = (rowId: number) => {
    const next = buildObjectFilterRow(nextRowId.current++);
    setObjectRows((rows) => {
      const rowIndex = rows.findIndex((row) => row.id === rowId);
      if (rowIndex < 0) return [...rows, next];
      const updated = rows.slice();
      updated.splice(rowIndex + 1, 0, next);
      return updated;
    });
  };

  const handleRemoveRow = (rowId: number) => {
    setObjectRows((rows) => {
      if (rows.length === 1) {
        return [buildObjectFilterRow(nextRowId.current++)];
      }
      return rows.filter((row) => row.id !== rowId);
    });
    setAppliedFingerprints((fingerprints) => {
      const next = new Map(fingerprints);
      next.delete(rowId);
      return next;
    });
    if (autocompleteRowId === rowId) {
      setAutocompleteRowId(null);
      setAutocompleteSuggestions([]);
      setAutocompleteIndex(0);
    }
  };

  const applyAutocompleteSuggestion = (rowId: number, suggestion: string) => {
    updateObjectRow(rowId, (row) => ({ ...row, path: suggestion }));
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
    requestAnimationFrame(() => {
      const input = pathInputRefs.current.get(rowId);
      if (!input) return;
      input.focus();
      input.setSelectionRange(suggestion.length, suggestion.length);
    });
  };

  const getActiveSuggestionForRow = useCallback(
    (rowId: number, inputValue: string) =>
      autocompleteRowId === rowId &&
      suppressInlineAutocompleteRowId !== rowId
        ? getInlineAutocompleteSuggestion(
            inputValue,
            autocompleteSuggestions,
            autocompleteIndex
          )
        : null,
    [
      autocompleteIndex,
      autocompleteRowId,
      autocompleteSuggestions,
      suppressInlineAutocompleteRowId
    ]
  );

  const acceptOpenAutocompleteSuggestion = useCallback(
    (rowId: number, options?: { refocusInput?: boolean }) => {
      if (autocompleteRowId !== rowId || autocompleteSuggestions.length === 0) {
        return false;
      }
      const suggestion = autocompleteSuggestions[autocompleteIndex];
      if (!suggestion) {
        return false;
      }
      updateObjectRow(rowId, (row) => ({ ...row, path: suggestion }));
      setAutocompleteRowId(null);
      setAutocompleteSuggestions([]);
      setAutocompleteIndex(0);
      setAutocompleteDropdownStyle(null);
      setSuppressInlineAutocompleteRowId(null);
      if (options?.refocusInput) {
        requestAnimationFrame(() => {
          const input = pathInputRefs.current.get(rowId);
          if (!input) return;
          input.focus();
          input.setSelectionRange(suggestion.length, suggestion.length);
        });
      }
      return true;
    },
    [autocompleteIndex, autocompleteRowId, autocompleteSuggestions, updateObjectRow]
  );

  const handleObjectRowEnter = (rowId: number, rowCanApply: boolean) => {
    const row = objectRows.find((candidate) => candidate.id === rowId);
    if (!row) return;
    if (!rowCanApply) return;
    void handleApplyRow(rowId);
  };

  const handleObjectPathKeyDown = (
    event: KeyboardEvent<HTMLInputElement>,
    rowId: number,
    rowCanApply: boolean
  ) => {
    if (event.key === "ArrowDown") {
      if (autocompleteRowId !== rowId || autocompleteSuggestions.length === 0) {
        return;
      }
      event.preventDefault();
      setAutocompleteIndex((index) =>
        Math.min(index + 1, autocompleteSuggestions.length - 1)
      );
      return;
    }
    if (event.key === "ArrowUp") {
      if (autocompleteRowId !== rowId || autocompleteSuggestions.length === 0) {
        return;
      }
      event.preventDefault();
      setAutocompleteIndex((index) => Math.max(index - 1, 0));
      return;
    }
    if (event.key === "Tab") {
      if (!event.shiftKey) {
        acceptOpenAutocompleteSuggestion(rowId);
      }
      return;
    }
    if (event.key === "Escape") {
      setAutocompleteRowId(null);
      setAutocompleteSuggestions([]);
      setAutocompleteIndex(0);
      setAutocompleteDropdownStyle(null);
      setSuppressInlineAutocompleteRowId(null);
      return;
    }
    if (event.key === "Backspace" || event.key === "Delete") {
      setSuppressInlineAutocompleteRowId(rowId);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (acceptOpenAutocompleteSuggestion(rowId, { refocusInput: true })) {
        return;
      }
      handleObjectRowEnter(rowId, rowCanApply);
    }
  };

  const handleObjectPathChange = (
    rowId: number,
    event: ChangeEvent<HTMLInputElement>
  ) => {
    const value = event.target.value;
    const nativeEvent = event.nativeEvent as InputEvent | undefined;
    if (
      nativeEvent?.inputType?.startsWith("insert") ||
      nativeEvent?.inputType === "historyRedo" ||
      nativeEvent?.inputType === "historyUndo"
    ) {
      setSuppressInlineAutocompleteRowId(null);
    }
    updateObjectRow(rowId, (current) => ({
      ...current,
      path: value
    }));
    requestAutocomplete(rowId, value);
    updateAutocompleteDropdownPosition(rowId);
  };

  const executableRows = useMemo(
    () =>
      objectRows.filter((row) => row.enabled).flatMap((row) => {
        const filter = toObjectSearchFilter(row);
        return filter ? [{ row, filter }] : [];
      }),
    [objectRows]
  );

  const handleApplyRow = async (rowId: number, pathOverride?: string) => {
    const row = objectRows.find((candidate) => candidate.id === rowId);
    if (!row) return;
    const effectiveRow =
      pathOverride && pathOverride !== row.path
        ? { ...row, path: pathOverride }
        : row;
    const filter = toObjectSearchFilter(effectiveRow);
    if (!filter) return;
    if (effectiveRow !== row) {
      updateObjectRow(rowId, (current) => ({ ...current, path: effectiveRow.path }));
    }
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
    await searchObjects(
      [filter],
      objectKeyCaseSensitive,
      objectValueCaseSensitive,
      searchScopePath
    );
    setAppliedFingerprints(
      new Map([[row.id, getObjectFilterFingerprint(effectiveRow)]])
    );
    setAppliedObjectKeyCaseSensitive(objectKeyCaseSensitive);
    setAppliedObjectValueCaseSensitive(objectValueCaseSensitive);
    setAppliedScopePath(searchScopePath.trim());
  };

  const handleApplyAll = async () => {
    if (executableRows.length === 0) {
      clearSearch();
      setAppliedFingerprints(new Map());
      return;
    }
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
    await searchObjects(
      executableRows.map(({ filter }) => filter),
      objectKeyCaseSensitive,
      objectValueCaseSensitive,
      searchScopePath
    );
    setAppliedFingerprints(
      new Map(
        executableRows.map(({ row }) => [row.id, getObjectFilterFingerprint(row)])
      )
    );
    setAppliedObjectKeyCaseSensitive(objectKeyCaseSensitive);
    setAppliedObjectValueCaseSensitive(objectValueCaseSensitive);
    setAppliedScopePath(searchScopePath.trim());
  };

  const handleResetObjectFilters = () => {
    const row = buildObjectFilterRow(nextRowId.current++);
    setObjectRows([row]);
    setAppliedFingerprints(new Map());
    setAppliedObjectKeyCaseSensitive(null);
    setAppliedObjectValueCaseSensitive(null);
    setAppliedScopePath("");
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
    if (activeSearchMode === "object") {
      clearSearch();
    }
  };

  useEffect(() => {
    return () => {
      clearTimeout(searchTimer.current);
      clearTimeout(suggestionTimer.current);
      clearTimeout(blurTimer.current);
    };
  }, []);

  useEffect(() => {
    if (searchMode !== "text" || !searchQuery) return;
    search(
      searchQuery,
      searchTarget,
      caseSensitive,
      useRegex,
      exactMatch,
      searchScopePath
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchTarget, caseSensitive, useRegex, exactMatch]);

  useEffect(() => {
    if (searchMode === "text" && searchQuery) {
      scheduleSearch(searchQuery, searchScopePath);
    }
  }, [scheduleSearch, searchMode, searchQuery, searchScopePath]);

  useEffect(() => {
    const resetUiState = () => {
      setSearchQuery("");
      setSearchTarget("both");
      setCaseSensitive(false);
      setUseRegex(false);
      setExactMatch(false);
      setObjectKeyCaseSensitive(false);
      setObjectValueCaseSensitive(false);
      setObjectRows([buildObjectFilterRow(1)]);
      setAppliedFingerprints(new Map());
      setAppliedObjectKeyCaseSensitive(null);
      setAppliedObjectValueCaseSensitive(null);
      setAppliedScopePath("");
      setAutocompleteRowId(null);
      setAutocompleteSuggestions([]);
      setAutocompleteIndex(0);
      setAutocompleteDropdownStyle(null);
      setSuppressInlineAutocompleteRowId(null);
      nextRowId.current = 2;
      setSearchMode("text");
      setSearchScopePath("");
      setSearchSort("relevance");
    };

    if (!isPersistentFilePath(filePath)) {
      restoredFilePath.current = filePath;
      resetUiState();
      return;
    }

    const persisted = loadPersistedSearchFilters(filePath);
    if (!persisted) {
      restoredFilePath.current = filePath;
      resetUiState();
      return;
    }

    setSearchQuery(persisted.searchQuery);
    setSearchTarget(persisted.searchTarget);
    setCaseSensitive(persisted.caseSensitive);
    setUseRegex(persisted.useRegex);
    setExactMatch(persisted.exactMatch);
    setObjectKeyCaseSensitive(persisted.objectKeyCaseSensitive);
    setObjectValueCaseSensitive(persisted.objectValueCaseSensitive);
    setObjectRows(persisted.objectRows);
    setAppliedFingerprints(new Map());
    setAppliedObjectKeyCaseSensitive(null);
    setAppliedObjectValueCaseSensitive(null);
    setAppliedScopePath("");
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
    nextRowId.current =
      Math.max(...persisted.objectRows.map((row) => row.id), 1) + 1;
    setSearchMode(persisted.searchMode);
    setSearchScopePath(persisted.searchScopePath);
    setSearchSort(persisted.searchSort);
    restoredFilePath.current = filePath;
  }, [filePath, setSearchMode, setSearchScopePath, setSearchSort]);

  useEffect(() => {
    if (!isPersistentFilePath(filePath)) return;
    if (restoredFilePath.current !== filePath) return;

    const payload: PersistedSearchFilters = {
      version: 1,
      searchMode,
      searchQuery,
      searchTarget,
      caseSensitive,
      useRegex,
      exactMatch,
      searchScopePath,
      searchSort,
      objectKeyCaseSensitive,
      objectValueCaseSensitive,
      objectRows
    };

    try {
      localStorage.setItem(
        getSearchFiltersStorageKey(filePath),
        JSON.stringify(payload)
      );
    } catch (err) {
      console.error("persist search filters error:", err);
    }
  }, [
    caseSensitive,
    exactMatch,
    filePath,
    objectKeyCaseSensitive,
    objectRows,
    objectValueCaseSensitive,
    searchMode,
    searchQuery,
    searchScopePath,
    searchSort,
    searchTarget,
    useRegex
  ]);

  useEffect(() => {
    if (autocompleteRowId === null) return;
    const row = objectRows.find((candidate) => candidate.id === autocompleteRowId);
    if (!row) return;
    const inlineSuggestion = getInlineAutocompleteSuggestion(
      row.path,
      autocompleteSuggestions,
      autocompleteIndex
    );
    if (!inlineSuggestion) return;
    const input = pathInputRefs.current.get(autocompleteRowId);
    if (!input || document.activeElement !== input) return;
    requestAnimationFrame(() => {
      if (document.activeElement !== input) return;
      input.setSelectionRange(row.path.length, inlineSuggestion.length);
    });
  }, [autocompleteIndex, autocompleteRowId, autocompleteSuggestions, objectRows]);

  useEffect(() => {
    if (autocompleteRowId === null || autocompleteSuggestions.length === 0) {
      setAutocompleteDropdownStyle(null);
      return;
    }
    updateAutocompleteDropdownPosition(autocompleteRowId);
    const handleViewportChange = () => {
      if (autocompleteRowId !== null) {
        updateAutocompleteDropdownPosition(autocompleteRowId);
      }
    };
    window.addEventListener("resize", handleViewportChange);
    window.addEventListener("scroll", handleViewportChange, true);
    return () => {
      window.removeEventListener("resize", handleViewportChange);
      window.removeEventListener("scroll", handleViewportChange, true);
    };
  }, [
    autocompleteRowId,
    autocompleteSuggestions.length,
    objectRows,
    updateAutocompleteDropdownPosition
  ]);

  const renderSegmentedControl = (
    options: readonly { value: string; labelKey: keyof typeof t }[],
    selected: string,
    onChange: (value: string) => void,
    name: string
  ) => (
    <div className="inline-flex rounded-xl border border-gray-200 bg-gray-100 p-1 shadow-sm dark:border-gray-700 dark:bg-gray-800/80">
      {options.map((option) => {
        const checked = selected === option.value;
        const label = t[option.labelKey] as string;
        return (
          <label key={option.value} className="min-w-0 flex-1 cursor-pointer">
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={checked}
              onChange={() => onChange(option.value)}
              className="peer sr-only"
            />
            <span
              className={`flex items-center justify-center rounded-lg px-3 py-2 text-xs font-semibold transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                checked
                  ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-gray-100"
                  : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
              }`}
            >
              {label}
            </span>
          </label>
        );
      })}
    </div>
  );

  return (
    <div className="border-b border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900">
      <div className="px-3 py-3 flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          {renderSegmentedControl(
            MODE_OPTIONS,
            searchMode,
            (mode) => setSearchMode(mode as "text" | "object"),
            "search-mode"
          )}
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative">
              <input
                id="search-path-input"
                type="text"
                placeholder={t.searchPathPlaceholder}
                value={searchScopePath}
                onChange={(event) => setSearchScopePath(event.target.value)}
                disabled={nodeCount === 0}
                className="w-[240px] rounded-lg border border-gray-200 bg-white px-3 py-2 pr-8 text-xs font-mono text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 disabled:cursor-not-allowed disabled:opacity-40 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-200 dark:placeholder:text-gray-500"
              />
              {searchScopePath && (
                <button
                  type="button"
                  onClick={handleClearScope}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 transition-colors hover:text-gray-600 dark:hover:text-gray-300"
                  title={t.clearSearchScope}
                >
                  <X size={12} />
                </button>
              )}
            </div>
          </div>
        </div>

        {searchMode === "text" ? (
          <div className="flex flex-col gap-3">
            <div className="relative">
              <Search
                size={14}
                className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400 dark:text-gray-500"
              />
              <input
                id="primary-search-input"
                type="text"
                placeholder={t.searchPlaceholder}
                value={searchQuery}
                onChange={(event) => handleSearchQueryChange(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    submitTextSearch();
                  }
                }}
                disabled={nodeCount === 0}
                className="w-full pl-8 pr-8 py-2 bg-white dark:bg-gray-700 border border-gray-300 dark:border-gray-600 rounded text-sm placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-blue-500 disabled:opacity-40 disabled:cursor-not-allowed text-gray-900 dark:text-gray-100"
              />
              {searchQuery && (
                <button
                  type="button"
                  onClick={handleClearTextSearch}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
                >
                  <X size={12} />
                </button>
              )}
            </div>

            <div className="flex flex-wrap items-start gap-3">
              <div className="flex flex-col gap-1.5">
                <div className="text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
                  {t.searchScope}
                </div>
                {renderSegmentedControl(
                  TEXT_TARGETS,
                  searchTarget,
                  setSearchTarget,
                  "search-target"
                )}
              </div>

              <div className="flex flex-col gap-1.5">
                <div className="text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
                  {t.searchFilters}
                </div>
                <div className="flex gap-2 flex-wrap">
                  {TEXT_FILTERS.map((filter) => {
                    const checked = filter.getChecked(filterState);
                    const label = t[filter.labelKey];
                    return (
                      <label key={filter.key} className="cursor-pointer">
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(event) =>
                            filter.onChange(event.target.checked, filterSetters)
                          }
                          className="peer sr-only"
                        />
                        <span
                          className={`inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-xs font-medium shadow-sm transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                            checked
                              ? "border-blue-500 bg-blue-50 text-blue-700 dark:border-blue-400/80 dark:bg-blue-500/15 dark:text-blue-200"
                              : "border-gray-200 bg-white text-gray-600 hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
                          }`}
                        >
                          <span
                            className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded border ${
                              checked
                                ? "border-blue-600 bg-blue-600 text-white dark:border-blue-300 dark:bg-blue-400 dark:text-gray-950"
                                : "border-gray-300 bg-white text-transparent dark:border-gray-600 dark:bg-gray-800"
                            }`}
                          >
                            <Check size={11} strokeWidth={3} />
                          </span>
                          {label}
                        </span>
                      </label>
                    );
                  })}
                </div>
              </div>

            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2 flex-wrap">
                <label className="cursor-pointer">
                  <input
                    type="checkbox"
                    checked={objectKeyCaseSensitive}
                    onChange={(event) =>
                      setObjectKeyCaseSensitive(event.target.checked)
                    }
                    className="peer sr-only"
                  />
                  <span
                    className={`inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-xs font-medium shadow-sm transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                      objectKeyCaseSensitive
                        ? "border-blue-500 bg-blue-50 text-blue-700 dark:border-blue-400/80 dark:bg-blue-500/15 dark:text-blue-200"
                        : "border-gray-200 bg-white text-gray-600 hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
                    }`}
                  >
                    <span
                      className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded border ${
                        objectKeyCaseSensitive
                          ? "border-blue-600 bg-blue-600 text-white dark:border-blue-300 dark:bg-blue-400 dark:text-gray-950"
                          : "border-gray-300 bg-white text-transparent dark:border-gray-600 dark:bg-gray-800"
                      }`}
                    >
                      <Check size={11} strokeWidth={3} />
                    </span>
                    {t.caseSensitiveKey}
                  </span>
                </label>

                <label className="cursor-pointer">
                  <input
                    type="checkbox"
                    checked={objectValueCaseSensitive}
                    onChange={(event) =>
                      setObjectValueCaseSensitive(event.target.checked)
                    }
                    className="peer sr-only"
                  />
                  <span
                    className={`inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-xs font-medium shadow-sm transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                      objectValueCaseSensitive
                        ? "border-blue-500 bg-blue-50 text-blue-700 dark:border-blue-400/80 dark:bg-blue-500/15 dark:text-blue-200"
                        : "border-gray-200 bg-white text-gray-600 hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
                    }`}
                  >
                    <span
                      className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded border ${
                        objectValueCaseSensitive
                          ? "border-blue-600 bg-blue-600 text-white dark:border-blue-300 dark:bg-blue-400 dark:text-gray-950"
                          : "border-gray-300 bg-white text-transparent dark:border-gray-600 dark:bg-gray-800"
                      }`}
                    >
                      <Check size={11} strokeWidth={3} />
                    </span>
                    {t.caseSensitiveValue}
                  </span>
                </label>
              </div>

              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={handleResetObjectFilters}
                  className="rounded-lg border border-gray-200 bg-white px-3 py-2 text-xs font-medium text-gray-600 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
                >
                  {t.resetFilters}
                </button>
                <button
                  type="button"
                  onClick={handleApplyAll}
                  disabled={nodeCount === 0 || searching || executableRows.length === 0}
                  className="rounded-lg bg-blue-600 px-3 py-2 text-xs font-semibold text-white shadow-sm transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {t.applyAll}
                </button>
              </div>
            </div>

            <div className="rounded-xl border border-gray-200 bg-white p-2 shadow-sm dark:border-gray-700 dark:bg-gray-800/70">
              <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
                {t.objectFilters}
              </div>
              <div className="max-h-44 overflow-auto app-scrollbar pr-1">
                <div className="flex flex-col gap-2">
                  {objectRows.map((row) => {
                    const isApplied =
                      activeSearchMode === "object" &&
                      appliedObjectKeyCaseSensitive === objectKeyCaseSensitive &&
                      appliedObjectValueCaseSensitive === objectValueCaseSensitive &&
                      appliedScopePath === searchScopePath.trim() &&
                      appliedFingerprints.get(row.id) ===
                        getObjectFilterFingerprint(row);
                    const activeSuggestions =
                      autocompleteRowId === row.id ? autocompleteSuggestions : [];
                    const inlineSuggestion = getActiveSuggestionForRow(row.id, row.path);
                    const rowCanApply = toObjectSearchFilter(row) !== null;
                    return (
                      <div
                        key={row.id}
                        className="grid grid-cols-[auto_minmax(220px,1.8fr)_150px_minmax(220px,1.4fr)_auto_auto_auto] items-center gap-2"
                      >
                        <label className="flex items-center justify-center">
                          <input
                            type="checkbox"
                            checked={row.enabled}
                            onChange={(event) =>
                              updateObjectRow(row.id, (current) => ({
                                ...current,
                                enabled: event.target.checked
                              }))
                            }
                            className="peer sr-only"
                          />
                          <span
                            className={`flex h-5 w-5 items-center justify-center rounded border ${
                              row.enabled
                                ? "border-blue-600 bg-blue-600 text-white dark:border-blue-300 dark:bg-blue-400 dark:text-gray-950"
                                : "border-gray-300 bg-white text-transparent dark:border-gray-600 dark:bg-gray-800"
                            }`}
                          >
                            <Check size={12} strokeWidth={3} />
                          </span>
                        </label>

                        <div className="relative">
                          <input
                            id={
                              row.id === primaryObjectRowId
                                ? "primary-search-input"
                                : undefined
                            }
                            ref={(element) => {
                              if (element) {
                                pathInputRefs.current.set(row.id, element);
                              } else {
                                pathInputRefs.current.delete(row.id);
                              }
                            }}
                            type="text"
                            role="combobox"
                            aria-autocomplete="both"
                            aria-expanded={activeSuggestions.length > 0}
                            aria-controls={`object-path-suggestions-${row.id}`}
                            aria-activedescendant={
                              activeSuggestions.length > 0
                                ? `object-path-suggestion-${row.id}-${autocompleteIndex}`
                                : undefined
                            }
                            autoComplete="off"
                            value={inlineSuggestion ?? row.path}
                            onFocus={() => requestAutocomplete(row.id, row.path)}
                            onBlur={() => {
                              clearTimeout(blurTimer.current);
                              blurTimer.current = setTimeout(() => {
                                setAutocompleteRowId(null);
                                setAutocompleteSuggestions([]);
                                setAutocompleteIndex(0);
                                setAutocompleteDropdownStyle(null);
                                setSuppressInlineAutocompleteRowId(null);
                              }, 120);
                            }}
                            onKeyDown={(event) =>
                              handleObjectPathKeyDown(event, row.id, rowCanApply)
                            }
                            onChange={(event) => handleObjectPathChange(row.id, event)}
                            placeholder={t.objectPathPlaceholder}
                            className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-xs font-mono text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200 dark:placeholder:text-gray-500"
                          />
                        </div>

                        <div className="relative">
                          <select
                            value={row.operator}
                            onChange={(event) =>
                              updateObjectRow(row.id, (current) => ({
                                ...current,
                                operator: event.target
                                  .value as ObjectSearchFilter["operator"]
                              }))
                            }
                            className="w-full appearance-none rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 pr-8 text-xs font-medium text-gray-700 shadow-sm outline-none transition-colors focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200"
                          >
                            {OBJECT_OPERATORS.map((operator) => (
                              <option key={operator.value} value={operator.value}>
                                {t[operator.labelKey]}
                              </option>
                            ))}
                          </select>
                          <ChevronDown
                            size={14}
                            className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 dark:text-gray-500"
                          />
                        </div>

                        <div className="flex items-center gap-2">
                          {row.operator !== "exists" && (
                            <input
                              type="text"
                              value={row.value}
                              onKeyDown={(event) => {
                                if (event.key === "Enter") {
                                  event.preventDefault();
                                  handleObjectRowEnter(row.id, rowCanApply);
                                }
                              }}
                              onChange={(event) =>
                                updateObjectRow(row.id, (current) => ({
                                  ...current,
                                  value: event.target.value
                                }))
                              }
                              placeholder={t.objectValuePlaceholder}
                              className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-xs text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200 dark:placeholder:text-gray-500"
                            />
                          )}
                          {isApplied && (
                            <span className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.12em] text-emerald-700 dark:text-emerald-300">
                              {t.applied}
                            </span>
                          )}
                        </div>

                        <button
                          type="button"
                          onClick={() => handleApplyRow(row.id)}
                          disabled={nodeCount === 0 || searching || !rowCanApply}
                          className="rounded-lg bg-blue-600 px-3 py-2 text-xs font-semibold text-white shadow-sm transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {t.apply}
                        </button>

                        <button
                          type="button"
                          onClick={() => handleRemoveRow(row.id)}
                          className="rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-gray-500 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 hover:text-gray-700 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:hover:border-gray-600 dark:hover:bg-gray-700/80 dark:hover:text-gray-200"
                          aria-label={t.resetFilters}
                        >
                          <Minus size={14} />
                        </button>

                        <button
                          type="button"
                          onClick={() => handleInsertRowAfter(row.id)}
                          className="rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-gray-500 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 hover:text-gray-700 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:hover:border-gray-600 dark:hover:bg-gray-700/80 dark:hover:text-gray-200"
                          aria-label={t.applyAll}
                        >
                          <Plus size={14} />
                        </button>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
      {autocompleteRowId !== null &&
        autocompleteSuggestions.length > 0 &&
        autocompleteDropdownStyle &&
        createPortal(
          <div
            id={`object-path-suggestions-${autocompleteRowId}`}
            role="listbox"
            className="z-[1200] overflow-hidden rounded-xl border border-gray-200 bg-white shadow-[0_12px_32px_rgba(15,23,42,0.2)] dark:border-gray-700 dark:bg-gray-900"
            style={{
              position: "absolute",
              top: autocompleteDropdownStyle.top,
              left: autocompleteDropdownStyle.left,
              width: autocompleteDropdownStyle.width
            }}
          >
            {autocompleteSuggestions.map((suggestion, suggestionIndex) => (
              <button
                id={`object-path-suggestion-${autocompleteRowId}-${suggestionIndex}`}
                key={suggestion}
                type="button"
                role="option"
                aria-selected={suggestionIndex === autocompleteIndex}
                tabIndex={-1}
                onMouseDown={(event) => {
                  event.preventDefault();
                  applyAutocompleteSuggestion(autocompleteRowId, suggestion);
                }}
                onMouseEnter={() => setAutocompleteIndex(suggestionIndex)}
                className={`flex w-full items-center justify-between border-b border-gray-100 px-3 py-2.5 text-left text-xs font-mono transition-colors last:border-b-0 dark:border-gray-800 ${
                  suggestionIndex === autocompleteIndex
                    ? "bg-blue-50 text-blue-700 dark:bg-blue-500/15 dark:text-blue-200"
                    : "text-gray-600 hover:bg-gray-50 dark:text-gray-300 dark:hover:bg-gray-800"
                }`}
              >
                {suggestion}
              </button>
            ))}
          </div>,
          document.body
        )}
    </div>
  );
};

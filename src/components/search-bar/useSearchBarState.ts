import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  ObjectSearchFilter,
  SearchMode,
  SearchSortMode
} from "../../store";
import { type RegexFlags } from "../RegexInput";
import {
  MAX_SUGGESTION_CACHE,
  PATH_SUGGESTION_LIMIT,
  buildObjectFilterRow,
  getInlineAutocompleteSuggestion,
  getObjectFilterFingerprint,
  getSearchFiltersStorageKey,
  isPersistentFilePath,
  loadPersistedSearchFilters,
  toObjectSearchFilter,
  type AutocompleteDropdownStyle,
  type FilterSetters,
  type FilterState,
  type ObjectFilterRow,
  type PersistedSearchFilters,
  type TextSearchTarget
} from "./shared";

interface UseSearchBarStateParams {
  filePath: string | null;
  searchMode: SearchMode;
  activeSearchMode: SearchMode | null;
  searchScopePath: string;
  searchSort: SearchSortMode;
  setSearchMode: (mode: SearchMode) => void;
  setSearchScopePath: (path: string) => void;
  setSearchSort: (sortMode: SearchSortMode) => void;
  search: (
    query: string,
    target: string,
    caseSensitive: boolean,
    useRegex: boolean,
    exactMatch: boolean,
    path: string,
    multiline?: boolean,
    dotAll?: boolean
  ) => Promise<void>;
  searchObjects: (
    filters: ObjectSearchFilter[],
    keyCaseSensitive: boolean,
    valueCaseSensitive: boolean,
    path: string
  ) => Promise<void>;
  clearSearch: () => void;
}

export function useSearchBarState({
  filePath,
  searchMode,
  activeSearchMode,
  searchScopePath,
  searchSort,
  setSearchMode,
  setSearchScopePath,
  setSearchSort,
  search,
  searchObjects,
  clearSearch
}: UseSearchBarStateParams) {
  const [searchQuery, setSearchQuery] = useState("");
  const [searchTarget, setSearchTarget] = useState<TextSearchTarget>("both");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [regexFlags, setRegexFlags] = useState<RegexFlags>({
    caseInsensitive: false,
    multiline: false,
    dotAll: false
  });
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
  const [appliedObjectKeyCaseSensitive, setAppliedObjectKeyCaseSensitive] =
    useState<boolean | null>(null);
  const [appliedObjectValueCaseSensitive, setAppliedObjectValueCaseSensitive] =
    useState<boolean | null>(null);
  const [appliedScopePath, setAppliedScopePath] = useState("");
  const [autocompleteRowId, setAutocompleteRowId] = useState<number | null>(
    null
  );
  const [autocompleteSuggestions, setAutocompleteSuggestions] = useState<
    string[]
  >([]);
  const [autocompleteIndex, setAutocompleteIndex] = useState(0);
  const [autocompleteDropdownStyle, setAutocompleteDropdownStyle] =
    useState<AutocompleteDropdownStyle | null>(null);
  const [suppressInlineAutocompleteRowId, setSuppressInlineAutocompleteRowId] =
    useState<number | null>(null);

  const searchTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const suggestionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const suggestionCache = useRef(new Map<string, string[]>());
  const pathInputRefs = useRef(new Map<number, HTMLInputElement>());
  const latestAutocompleteRequest = useRef<{
    rowId: number;
    prefix: string;
  } | null>(null);
  const restoredFilePath = useRef<string | null>(null);
  const nextRowId = useRef(2);

  const filterState: FilterState = { caseSensitive, useRegex, exactMatch };
  const filterSetters: FilterSetters = {
    setCaseSensitive,
    setUseRegex,
    setExactMatch
  };
  const primaryObjectRowId = objectRows[0]?.id ?? null;

  const clearAutocompleteState = useCallback(() => {
    setAutocompleteRowId(null);
    setAutocompleteSuggestions([]);
    setAutocompleteIndex(0);
    setAutocompleteDropdownStyle(null);
    setSuppressInlineAutocompleteRowId(null);
  }, []);

  const scheduleSearch = useCallback(
    (query: string, path: string) => {
      clearTimeout(searchTimer.current);
      if (!query.trim()) {
        clearSearch();
        return;
      }

      const effectiveCaseSensitive = useRegex
        ? !regexFlags.caseInsensitive
        : caseSensitive;

      searchTimer.current = setTimeout(() => {
        search(
          query,
          searchTarget,
          effectiveCaseSensitive,
          useRegex,
          exactMatch,
          path,
          regexFlags.multiline,
          regexFlags.dotAll
        );
      }, 150);
    },
    [
      caseSensitive,
      clearSearch,
      exactMatch,
      regexFlags,
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

          if (suggestionCache.current.size >= MAX_SUGGESTION_CACHE) {
            const firstKey = suggestionCache.current.keys().next().value;
            if (firstKey !== undefined) {
              suggestionCache.current.delete(firstKey);
            }
          }

          suggestionCache.current.set(trimmed, suggestions);
          setAutocompleteRowId(rowId);
          setAutocompleteSuggestions(suggestions);
          setAutocompleteIndex(0);
        } catch (error) {
          console.error("suggest_property_paths error:", error);
          clearAutocompleteState();
        }
      }, 80);
    },
    [clearAutocompleteState]
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

  const registerPathInput = useCallback(
    (rowId: number, element: HTMLInputElement | null) => {
      if (element) {
        pathInputRefs.current.set(rowId, element);
      } else {
        pathInputRefs.current.delete(rowId);
      }
    },
    []
  );

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

  const handleClearTextSearch = useCallback(() => {
    clearTimeout(searchTimer.current);
    setSearchQuery("");
    clearSearch();
  }, [clearSearch]);

  const handleClearScope = useCallback(() => {
    setSearchScopePath("");
  }, [setSearchScopePath]);

  const submitTextSearch = useCallback(() => {
    const query = searchQuery.trim();
    if (!query) {
      clearSearch();
      return;
    }

    clearTimeout(searchTimer.current);
    const effectiveCaseSensitive = useRegex
      ? !regexFlags.caseInsensitive
      : caseSensitive;

    void search(
      query,
      searchTarget,
      effectiveCaseSensitive,
      useRegex,
      exactMatch,
      searchScopePath,
      regexFlags.multiline,
      regexFlags.dotAll
    );
  }, [
    caseSensitive,
    clearSearch,
    exactMatch,
    regexFlags,
    search,
    searchQuery,
    searchScopePath,
    searchTarget,
    useRegex
  ]);

  const handleInsertRowAfter = useCallback((rowId: number) => {
    const nextRow = buildObjectFilterRow(nextRowId.current++);
    setObjectRows((rows) => {
      const rowIndex = rows.findIndex((row) => row.id === rowId);
      if (rowIndex < 0) return [...rows, nextRow];

      const updated = rows.slice();
      updated.splice(rowIndex + 1, 0, nextRow);
      return updated;
    });
  }, []);

  const handleRemoveRow = useCallback(
    (rowId: number) => {
      setObjectRows((rows) => {
        if (rows.length === 1) {
          return [buildObjectFilterRow(nextRowId.current++)];
        }
        return rows.filter((row) => row.id !== rowId);
      });

      setAppliedFingerprints((fingerprints) => {
        const nextFingerprints = new Map(fingerprints);
        nextFingerprints.delete(rowId);
        return nextFingerprints;
      });

      if (autocompleteRowId === rowId) {
        clearAutocompleteState();
      }
    },
    [autocompleteRowId, clearAutocompleteState]
  );

  const applyAutocompleteSuggestion = useCallback(
    (rowId: number, suggestion: string) => {
      updateObjectRow(rowId, (row) => ({ ...row, path: suggestion }));
      clearAutocompleteState();

      requestAnimationFrame(() => {
        const input = pathInputRefs.current.get(rowId);
        if (!input) return;
        input.focus();
        input.setSelectionRange(suggestion.length, suggestion.length);
      });
    },
    [clearAutocompleteState, updateObjectRow]
  );

  const getActiveSuggestionForRow = useCallback(
    (rowId: number, inputValue: string) =>
      autocompleteRowId === rowId && suppressInlineAutocompleteRowId !== rowId
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
      clearAutocompleteState();

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
    [
      autocompleteIndex,
      autocompleteRowId,
      autocompleteSuggestions,
      clearAutocompleteState,
      updateObjectRow
    ]
  );

  const handleObjectPathFocus = useCallback(
    (rowId: number, path: string) => {
      requestAutocomplete(rowId, path);
    },
    [requestAutocomplete]
  );

  const handleObjectPathBlur = useCallback(() => {
    clearTimeout(blurTimer.current);
    blurTimer.current = setTimeout(() => {
      clearAutocompleteState();
    }, 120);
  }, [clearAutocompleteState]);

  const handleObjectPathChange = useCallback(
    (rowId: number, event: ChangeEvent<HTMLInputElement>) => {
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
    },
    [requestAutocomplete, updateAutocompleteDropdownPosition, updateObjectRow]
  );

  const handleToggleObjectRowEnabled = useCallback(
    (rowId: number, enabled: boolean) => {
      updateObjectRow(rowId, (current) => ({ ...current, enabled }));
    },
    [updateObjectRow]
  );

  const handleObjectOperatorChange = useCallback(
    (rowId: number, operator: ObjectSearchFilter["operator"]) => {
      updateObjectRow(rowId, (current) => ({ ...current, operator }));
    },
    [updateObjectRow]
  );

  const handleObjectValueChange = useCallback(
    (rowId: number, value: string) => {
      updateObjectRow(rowId, (current) => ({ ...current, value }));
    },
    [updateObjectRow]
  );

  const handleObjectRegexFlagsChange = useCallback(
    (rowId: number, flags: RegexFlags) => {
      updateObjectRow(rowId, (current) => ({
        ...current,
        regexCaseInsensitive: flags.caseInsensitive,
        regexMultiline: flags.multiline,
        regexDotAll: flags.dotAll
      }));
    },
    [updateObjectRow]
  );

  const executableRows = objectRows
    .filter((row) => row.enabled)
    .flatMap((row) => {
      const filter = toObjectSearchFilter(row);
      return filter ? [{ row, filter }] : [];
    });

  const handleApplyRow = useCallback(
    async (rowId: number, pathOverride?: string) => {
      const row = objectRows.find((candidate) => candidate.id === rowId);
      if (!row) return;

      const effectiveRow =
        pathOverride && pathOverride !== row.path
          ? { ...row, path: pathOverride }
          : row;
      const filter = toObjectSearchFilter(effectiveRow);
      if (!filter) return;

      if (effectiveRow !== row) {
        updateObjectRow(rowId, (current) => ({
          ...current,
          path: effectiveRow.path
        }));
      }

      clearAutocompleteState();
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
    },
    [
      clearAutocompleteState,
      objectKeyCaseSensitive,
      objectRows,
      objectValueCaseSensitive,
      searchObjects,
      searchScopePath,
      updateObjectRow
    ]
  );

  const handleApplyAll = useCallback(async () => {
    if (executableRows.length === 0) {
      clearSearch();
      setAppliedFingerprints(new Map());
      return;
    }

    clearAutocompleteState();
    await searchObjects(
      executableRows.map(({ filter }) => filter),
      objectKeyCaseSensitive,
      objectValueCaseSensitive,
      searchScopePath
    );
    setAppliedFingerprints(
      new Map(
        executableRows.map(({ row }) => [
          row.id,
          getObjectFilterFingerprint(row)
        ])
      )
    );
    setAppliedObjectKeyCaseSensitive(objectKeyCaseSensitive);
    setAppliedObjectValueCaseSensitive(objectValueCaseSensitive);
    setAppliedScopePath(searchScopePath.trim());
  }, [
    clearAutocompleteState,
    clearSearch,
    executableRows,
    objectKeyCaseSensitive,
    objectValueCaseSensitive,
    searchObjects,
    searchScopePath
  ]);

  const handleResetObjectFilters = useCallback(() => {
    const row = buildObjectFilterRow(nextRowId.current++);
    setObjectRows([row]);
    setAppliedFingerprints(new Map());
    setAppliedObjectKeyCaseSensitive(null);
    setAppliedObjectValueCaseSensitive(null);
    setAppliedScopePath("");
    clearAutocompleteState();

    if (activeSearchMode === "object") {
      clearSearch();
    }
  }, [activeSearchMode, clearAutocompleteState, clearSearch]);

  const resetUiState = useCallback(() => {
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
    clearAutocompleteState();
    nextRowId.current = 2;
    setSearchMode("text");
    setSearchScopePath("");
    setSearchSort("relevance");
  }, [
    clearAutocompleteState,
    setSearchMode,
    setSearchScopePath,
    setSearchSort
  ]);

  const handleObjectPathKeyDown = (
    event: KeyboardEvent<HTMLInputElement>,
    rowId: number
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
      clearAutocompleteState();
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
      void handleApplyRow(rowId);
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

    const effectiveCaseSensitive = useRegex
      ? !regexFlags.caseInsensitive
      : caseSensitive;

    search(
      searchQuery,
      searchTarget,
      effectiveCaseSensitive,
      useRegex,
      exactMatch,
      searchScopePath,
      regexFlags.multiline,
      regexFlags.dotAll
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchTarget, caseSensitive, useRegex, exactMatch, regexFlags]);

  useEffect(() => {
    if (searchMode === "text" && searchQuery) {
      scheduleSearch(searchQuery, searchScopePath);
    }
  }, [scheduleSearch, searchMode, searchQuery, searchScopePath]);

  useEffect(() => {
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
    clearAutocompleteState();
    nextRowId.current =
      Math.max(...persisted.objectRows.map((row) => row.id), 1) + 1;
    setSearchMode(persisted.searchMode);
    setSearchScopePath(persisted.searchScopePath);
    setSearchSort(persisted.searchSort);
    restoredFilePath.current = filePath;
  }, [
    clearAutocompleteState,
    filePath,
    resetUiState,
    setSearchMode,
    setSearchScopePath,
    setSearchSort
  ]);

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
    } catch (error) {
      console.error("persist search filters error:", error);
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

    const row = objectRows.find(
      (candidate) => candidate.id === autocompleteRowId
    );
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
  }, [
    autocompleteIndex,
    autocompleteRowId,
    autocompleteSuggestions,
    objectRows
  ]);

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

  return {
    searchQuery,
    searchTarget,
    caseSensitive,
    useRegex,
    regexFlags,
    exactMatch,
    objectKeyCaseSensitive,
    objectValueCaseSensitive,
    objectRows,
    appliedFingerprints,
    appliedObjectKeyCaseSensitive,
    appliedObjectValueCaseSensitive,
    appliedScopePath,
    autocompleteRowId,
    autocompleteSuggestions,
    autocompleteIndex,
    autocompleteDropdownStyle,
    filterState,
    filterSetters,
    primaryObjectRowId,
    executableRowCount: executableRows.length,
    setRegexFlags,
    setSearchTarget,
    setAutocompleteIndex,
    setObjectKeyCaseSensitive,
    setObjectValueCaseSensitive,
    handleSearchQueryChange,
    handleClearTextSearch,
    handleClearScope,
    submitTextSearch,
    registerPathInput,
    getActiveSuggestionForRow,
    handleResetObjectFilters,
    handleApplyAll,
    handleToggleObjectRowEnabled,
    handleObjectPathFocus,
    handleObjectPathBlur,
    handleObjectPathKeyDown,
    handleObjectPathChange,
    handleObjectOperatorChange,
    handleObjectValueChange,
    handleObjectRegexFlagsChange,
    handleApplyRow,
    handleRemoveRow,
    handleInsertRowAfter,
    applyAutocompleteSuggestion
  };
}

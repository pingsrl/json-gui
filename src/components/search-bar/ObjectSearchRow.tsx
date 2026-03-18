import { type ChangeEvent, type FC, type KeyboardEvent } from "react";
import { Check, ChevronDown, Minus, Plus } from "lucide-react";
import { useI18n } from "../../i18n";
import { type ObjectSearchFilter } from "../../store";
import { RegexInput, type RegexFlags } from "../RegexInput";
import {
  OBJECT_OPERATORS,
  toObjectSearchFilter,
  type ObjectFilterRow
} from "./shared";

interface ObjectSearchRowProps {
  row: ObjectFilterRow;
  nodeCount: number;
  searching: boolean;
  isApplied: boolean;
  primary: boolean;
  activeSuggestions: string[];
  autocompleteIndex: number;
  inlineSuggestion: string | null;
  registerPathInput: (rowId: number, element: HTMLInputElement | null) => void;
  onToggleEnabled: (rowId: number, enabled: boolean) => void;
  onPathFocus: (rowId: number, path: string) => void;
  onPathBlur: () => void;
  onPathKeyDown: (
    event: KeyboardEvent<HTMLInputElement>,
    rowId: number
  ) => void;
  onPathChange: (rowId: number, event: ChangeEvent<HTMLInputElement>) => void;
  onOperatorChange: (
    rowId: number,
    operator: ObjectSearchFilter["operator"]
  ) => void;
  onValueChange: (rowId: number, value: string) => void;
  onRegexFlagsChange: (rowId: number, flags: RegexFlags) => void;
  onApply: (rowId: number) => void;
  onRemove: (rowId: number) => void;
  onInsertAfter: (rowId: number) => void;
}

export const ObjectSearchRow: FC<ObjectSearchRowProps> = ({
  row,
  nodeCount,
  searching,
  isApplied,
  primary,
  activeSuggestions,
  autocompleteIndex,
  inlineSuggestion,
  registerPathInput,
  onToggleEnabled,
  onPathFocus,
  onPathBlur,
  onPathKeyDown,
  onPathChange,
  onOperatorChange,
  onValueChange,
  onRegexFlagsChange,
  onApply,
  onRemove,
  onInsertAfter
}) => {
  const { t } = useI18n();
  const rowCanApply = toObjectSearchFilter(row) !== null;

  return (
    <div className="grid grid-cols-[auto_minmax(220px,1.8fr)_150px_minmax(220px,1.4fr)_auto_auto_auto] items-center gap-2">
      <label className="flex items-center justify-center">
        <input
          type="checkbox"
          checked={row.enabled}
          onChange={(event) => onToggleEnabled(row.id, event.target.checked)}
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
          id={primary ? "primary-search-input" : undefined}
          ref={(element) => registerPathInput(row.id, element)}
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
          onFocus={() => onPathFocus(row.id, row.path)}
          onBlur={onPathBlur}
          onKeyDown={(event) => onPathKeyDown(event, row.id)}
          onChange={(event) => onPathChange(row.id, event)}
          placeholder={t.objectPathPlaceholder}
          className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-xs font-mono text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200 dark:placeholder:text-gray-500"
        />
      </div>

      <div className="relative">
        <select
          value={row.operator}
          onChange={(event) =>
            onOperatorChange(
              row.id,
              event.target.value as ObjectSearchFilter["operator"]
            )
          }
          className="w-full appearance-none rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 pr-8 text-xs font-medium text-gray-700 shadow-sm outline-none transition-colors focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200"
        >
          {OBJECT_OPERATORS.map((operator) => (
            <option key={operator.value} value={operator.value}>
              {t[operator.labelKey] as string}
            </option>
          ))}
        </select>
        <ChevronDown
          size={14}
          className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 dark:text-gray-500"
        />
      </div>

      <div className="flex items-center gap-2">
        {row.operator === "regex" ? (
          <div className="flex-1">
            <RegexInput
              compact
              value={row.value}
              flags={{
                caseInsensitive: row.regexCaseInsensitive,
                multiline: row.regexMultiline,
                dotAll: row.regexDotAll
              }}
              placeholder={t.objectValuePlaceholder}
              disabled={nodeCount === 0}
              onChange={(value) => onValueChange(row.id, value)}
              onFlagsChange={(flags) => onRegexFlagsChange(row.id, flags)}
              onClear={() => onValueChange(row.id, "")}
              onEnter={() => onApply(row.id)}
            />
          </div>
        ) : row.operator !== "exists" ? (
          <input
            type="text"
            value={row.value}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                onApply(row.id);
              }
            }}
            onChange={(event) => onValueChange(row.id, event.target.value)}
            placeholder={t.objectValuePlaceholder}
            className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-xs text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-200 dark:placeholder:text-gray-500"
          />
        ) : null}
        {isApplied && (
          <span className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.12em] text-emerald-700 dark:text-emerald-300">
            {t.applied}
          </span>
        )}
      </div>

      <button
        type="button"
        onClick={() => onApply(row.id)}
        disabled={nodeCount === 0 || searching || !rowCanApply}
        className="rounded-lg bg-blue-600 px-3 py-2 text-xs font-semibold text-white shadow-sm transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {t.apply}
      </button>

      <button
        type="button"
        onClick={() => onRemove(row.id)}
        className="rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-gray-500 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 hover:text-gray-700 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:hover:border-gray-600 dark:hover:bg-gray-700/80 dark:hover:text-gray-200"
        aria-label={t.resetFilters}
      >
        <Minus size={14} />
      </button>

      <button
        type="button"
        onClick={() => onInsertAfter(row.id)}
        className="rounded-lg border border-gray-200 bg-white px-2.5 py-2 text-gray-500 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 hover:text-gray-700 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:hover:border-gray-600 dark:hover:bg-gray-700/80 dark:hover:text-gray-200"
        aria-label={t.applyAll}
      >
        <Plus size={14} />
      </button>
    </div>
  );
};

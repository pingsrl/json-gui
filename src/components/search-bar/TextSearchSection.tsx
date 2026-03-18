import { type FC } from "react";
import { Search, X } from "lucide-react";
import { useI18n } from "../../i18n";
import { RegexInput, type RegexFlags } from "../RegexInput";
import { SegmentedControl } from "./SegmentedControl";
import { ToggleChip } from "./ToggleChip";
import {
  TEXT_MODIFIERS,
  TEXT_REGEX_FILTER,
  TEXT_TARGETS,
  type FilterSetters,
  type FilterState,
  type TextSearchTarget
} from "./shared";

interface TextSearchSectionProps {
  nodeCount: number;
  searchQuery: string;
  searchTarget: TextSearchTarget;
  useRegex: boolean;
  caseSensitive: boolean;
  exactMatch: boolean;
  regexFlags: RegexFlags;
  filterState: FilterState;
  filterSetters: FilterSetters;
  onSearchQueryChange: (query: string) => void;
  onSearchTargetChange: (target: TextSearchTarget) => void;
  onRegexFlagsChange: (flags: RegexFlags) => void;
  onClearTextSearch: () => void;
  onSubmitTextSearch: () => void;
}

export const TextSearchSection: FC<TextSearchSectionProps> = ({
  nodeCount,
  searchQuery,
  searchTarget,
  useRegex,
  caseSensitive,
  exactMatch,
  regexFlags,
  filterState,
  filterSetters,
  onSearchQueryChange,
  onSearchTargetChange,
  onRegexFlagsChange,
  onClearTextSearch,
  onSubmitTextSearch
}) => {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-3">
      {useRegex ? (
        <RegexInput
          id="primary-search-input"
          value={searchQuery}
          flags={regexFlags}
          placeholder={t.searchPlaceholder}
          disabled={nodeCount === 0}
          onChange={onSearchQueryChange}
          onFlagsChange={onRegexFlagsChange}
          onClear={onClearTextSearch}
          onEnter={onSubmitTextSearch}
        />
      ) : (
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
            onChange={(event) => onSearchQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                onSubmitTextSearch();
              }
            }}
            disabled={nodeCount === 0}
            className="w-full rounded border border-gray-300 bg-white py-2 pl-8 pr-8 text-sm text-gray-900 placeholder-gray-400 focus:border-blue-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-40 dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100 dark:placeholder-gray-500"
          />
          {searchQuery && (
            <button
              type="button"
              onClick={onClearTextSearch}
              className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            >
              <X size={12} />
            </button>
          )}
        </div>
      )}

      <div className="flex flex-wrap items-start gap-3">
        <div className="flex flex-col gap-1.5">
          <div className="text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchScope}
          </div>
          <SegmentedControl
            options={TEXT_TARGETS.map((option) => ({
              value: option.value,
              label: t[option.labelKey] as string
            }))}
            selected={searchTarget}
            onChange={(target) =>
              onSearchTargetChange(target as TextSearchTarget)
            }
            name="search-target"
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <div className="text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchFilters}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {TEXT_MODIFIERS.map((filter) => {
              const checked = filter.getChecked(filterState);
              return (
                <ToggleChip
                  key={filter.key}
                  checked={checked}
                  dimmed={useRegex && !checked}
                  label={t[filter.labelKey] as string}
                  onChange={(nextChecked) =>
                    filter.onChange(nextChecked, filterSetters)
                  }
                />
              );
            })}

            <div className="h-5 w-px bg-gray-200 dark:bg-gray-700" />

            <ToggleChip
              checked={TEXT_REGEX_FILTER.getChecked(filterState)}
              dimmed={(caseSensitive || exactMatch) && !useRegex}
              label={t[TEXT_REGEX_FILTER.labelKey] as string}
              onChange={(nextChecked) =>
                TEXT_REGEX_FILTER.onChange(nextChecked, filterSetters)
              }
            />
          </div>
        </div>
      </div>
    </div>
  );
};

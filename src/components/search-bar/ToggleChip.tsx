import { type FC } from "react";
import { Check } from "lucide-react";

interface ToggleChipProps {
  checked: boolean;
  label: string;
  onChange: (checked: boolean) => void;
  dimmed?: boolean;
}

export const ToggleChip: FC<ToggleChipProps> = ({
  checked,
  label,
  onChange,
  dimmed = false
}) => (
  <label
    className={`cursor-pointer transition-opacity ${dimmed ? "opacity-35" : ""}`}
  >
    <input
      type="checkbox"
      checked={checked}
      onChange={(event) => onChange(event.target.checked)}
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

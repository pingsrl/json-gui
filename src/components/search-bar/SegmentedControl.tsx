import { type FC } from "react";

interface SegmentedControlOption {
  value: string;
  label: string;
}

interface SegmentedControlProps {
  options: readonly SegmentedControlOption[];
  selected: string;
  onChange: (value: string) => void;
  name: string;
}

export const SegmentedControl: FC<SegmentedControlProps> = ({
  options,
  selected,
  onChange,
  name
}) => (
  <div className="inline-flex rounded-xl border border-gray-200 bg-gray-100 p-1 shadow-sm dark:border-gray-700 dark:bg-gray-800/80">
    {options.map((option) => {
      const checked = selected === option.value;
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
            {option.label}
          </span>
        </label>
      );
    })}
  </div>
);
